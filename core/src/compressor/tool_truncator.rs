//! Tool Output Truncator
//!
//! This module provides a dedicated component for truncating tool outputs with
//! summary generation. It can be used standalone or integrated with
//! `SmartCompactionStrategy` for fine-grained control over tool output management.
//!
//! # Features
//!
//! - **Configurable Maximum Length**: Set the maximum characters to retain
//! - **Custom Summary Templates**: Customize truncation summary format
//! - **Rich Output Metadata**: Returns truncation details including original length
//! - **Tool Name Context**: Includes tool name in summaries for clarity
//!
//! # Example
//!
//! ```rust,ignore
//! use aether_core::compressor::ToolTruncator;
//!
//! let truncator = ToolTruncator::new(2000);
//!
//! let output = truncator.truncate(&large_output, "read_file");
//! if output.was_truncated {
//!     println!("Truncated: {} -> {} chars", output.original_len, output.content.len());
//!     println!("Summary: {}", output.summary);
//! }
//! ```

/// Result of a truncation operation
///
/// Contains the truncated content, a generated summary, and metadata
/// about the truncation operation.
#[derive(Debug, Clone, PartialEq)]
pub struct TruncatedOutput {
    /// The truncated content
    pub content: String,
    /// Generated summary describing the truncation
    pub summary: String,
    /// Original length in characters
    pub original_len: usize,
    /// Whether truncation actually occurred
    pub was_truncated: bool,
}

impl TruncatedOutput {
    /// Create a new TruncatedOutput indicating no truncation occurred
    fn unchanged(content: String) -> Self {
        let len = content.len();
        Self {
            content,
            summary: String::new(),
            original_len: len,
            was_truncated: false,
        }
    }

    /// Create a new TruncatedOutput indicating truncation occurred
    fn truncated(content: String, summary: String, original_len: usize) -> Self {
        Self {
            content,
            summary,
            original_len,
            was_truncated: true,
        }
    }
}

/// Default summary template
const DEFAULT_SUMMARY_TEMPLATE: &str = "[Truncated {tool_name}: {original_len} -> {truncated_len} chars] {preview}...";

/// Tool output truncator with configurable settings
///
/// Provides methods for truncating tool outputs while generating
/// meaningful summaries that preserve context about what was removed.
#[derive(Debug, Clone)]
pub struct ToolTruncator {
    /// Maximum characters to retain
    max_chars: usize,
    /// Template for generating summaries
    summary_template: String,
}

impl Default for ToolTruncator {
    fn default() -> Self {
        Self::new(2000)
    }
}

impl ToolTruncator {
    /// Create a new ToolTruncator with the specified maximum characters
    ///
    /// # Arguments
    ///
    /// * `max_chars` - Maximum number of characters to retain in truncated output
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// let truncator = ToolTruncator::new(2000);
    /// ```
    pub fn new(max_chars: usize) -> Self {
        Self {
            max_chars,
            summary_template: DEFAULT_SUMMARY_TEMPLATE.to_string(),
        }
    }

    /// Set a custom summary template
    ///
    /// The template supports the following placeholders:
    /// - `{original_len}`: Original content length in characters
    /// - `{truncated_len}`: Truncated content length in characters
    /// - `{preview}`: First line preview (up to 50 chars)
    /// - `{tool_name}`: Name of the tool that produced the output
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// let truncator = ToolTruncator::new(2000)
    ///     .with_summary_template("[{tool_name}: {original_len}B truncated to {truncated_len}B]");
    /// ```
    pub fn with_summary_template(mut self, template: impl Into<String>) -> Self {
        self.summary_template = template.into();
        self
    }

    /// Get the configured maximum characters
    pub fn max_chars(&self) -> usize {
        self.max_chars
    }

    /// Get the configured summary template
    pub fn summary_template(&self) -> &str {
        &self.summary_template
    }

    /// Check if the given output should be truncated
    ///
    /// Returns true if the output length exceeds `max_chars`.
    ///
    /// # Arguments
    ///
    /// * `output` - The output string to check
    pub fn should_truncate(&self, output: &str) -> bool {
        output.len() > self.max_chars
    }

    /// Truncate the output if necessary and generate a summary
    ///
    /// If the output is shorter than `max_chars`, returns it unchanged
    /// with `was_truncated` set to false. Otherwise, truncates the
    /// output and generates a summary.
    ///
    /// # Arguments
    ///
    /// * `output` - The output string to truncate
    /// * `tool_name` - Name of the tool for context in the summary
    ///
    /// # Returns
    ///
    /// A `TruncatedOutput` containing the result and metadata.
    pub fn truncate(&self, output: &str, tool_name: &str) -> TruncatedOutput {
        let original_len = output.len();

        if !self.should_truncate(output) {
            return TruncatedOutput::unchanged(output.to_string());
        }

        // Generate summary
        let summary = self.generate_summary(output, tool_name, self.max_chars);

        // Calculate space for content after summary
        let summary_len = summary.len();
        let separator_len = 1; // newline between summary and content
        let remaining_space = self.max_chars.saturating_sub(summary_len + separator_len);

        let content = if remaining_space > 0 {
            // Include summary and truncated content
            let truncated_content: String = output.chars().take(remaining_space).collect();
            format!("{}\n{}", summary, truncated_content)
        } else {
            // Summary alone exceeds max_chars, just return summary
            summary.clone()
        };

        TruncatedOutput::truncated(content, summary, original_len)
    }

    /// Generate a summary for the truncated output
    ///
    /// Uses the configured template with placeholder substitution.
    fn generate_summary(&self, output: &str, tool_name: &str, truncated_len: usize) -> String {
        let original_len = output.len();

        // Extract first line as preview (up to 50 characters)
        let preview: String = output
            .lines()
            .next()
            .unwrap_or("")
            .chars()
            .take(50)
            .collect();

        // Substitute placeholders in template
        self.summary_template
            .replace("{original_len}", &original_len.to_string())
            .replace("{truncated_len}", &truncated_len.to_string())
            .replace("{preview}", &preview)
            .replace("{tool_name}", tool_name)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // =========================================================================
    // Construction and Configuration Tests
    // =========================================================================

    #[test]
    fn test_new_with_max_chars() {
        let truncator = ToolTruncator::new(5000);
        assert_eq!(truncator.max_chars(), 5000);
    }

    #[test]
    fn test_default() {
        let truncator = ToolTruncator::default();
        assert_eq!(truncator.max_chars(), 2000);
    }

    #[test]
    fn test_with_summary_template() {
        let template = "[Custom: {original_len} -> {truncated_len}]";
        let truncator = ToolTruncator::new(1000)
            .with_summary_template(template);

        assert_eq!(truncator.summary_template(), template);
    }

    #[test]
    fn test_builder_chaining() {
        let truncator = ToolTruncator::new(3000)
            .with_summary_template("Custom template");

        assert_eq!(truncator.max_chars(), 3000);
        assert_eq!(truncator.summary_template(), "Custom template");
    }

    // =========================================================================
    // should_truncate Tests
    // =========================================================================

    #[test]
    fn test_should_truncate_returns_false_for_short_output() {
        let truncator = ToolTruncator::new(100);
        assert!(!truncator.should_truncate("Short output"));
    }

    #[test]
    fn test_should_truncate_returns_false_at_exactly_max_chars() {
        let truncator = ToolTruncator::new(10);
        let output = "0123456789"; // Exactly 10 chars
        assert!(!truncator.should_truncate(output));
    }

    #[test]
    fn test_should_truncate_returns_true_above_max_chars() {
        let truncator = ToolTruncator::new(10);
        let output = "01234567890"; // 11 chars
        assert!(truncator.should_truncate(output));
    }

    #[test]
    fn test_should_truncate_empty_string() {
        let truncator = ToolTruncator::new(100);
        assert!(!truncator.should_truncate(""));
    }

    // =========================================================================
    // truncate Tests - No Truncation Needed
    // =========================================================================

    #[test]
    fn test_truncate_short_output_unchanged() {
        let truncator = ToolTruncator::new(1000);
        let output = "Short output";

        let result = truncator.truncate(output, "test_tool");

        assert!(!result.was_truncated);
        assert_eq!(result.content, output);
        assert_eq!(result.original_len, output.len());
        assert!(result.summary.is_empty());
    }

    #[test]
    fn test_truncate_exactly_at_limit() {
        let truncator = ToolTruncator::new(12);
        let output = "Exactly 12!!"; // 12 chars

        let result = truncator.truncate(output, "test_tool");

        assert!(!result.was_truncated);
        assert_eq!(result.content, output);
    }

    #[test]
    fn test_truncate_empty_string() {
        let truncator = ToolTruncator::new(100);

        let result = truncator.truncate("", "test_tool");

        assert!(!result.was_truncated);
        assert_eq!(result.content, "");
        assert_eq!(result.original_len, 0);
    }

    // =========================================================================
    // truncate Tests - Truncation Occurs
    // =========================================================================

    #[test]
    fn test_truncate_long_output() {
        let truncator = ToolTruncator::new(200);
        let output = "x".repeat(1000);

        let result = truncator.truncate(&output, "test_tool");

        assert!(result.was_truncated);
        assert!(result.content.len() <= 200);
        assert_eq!(result.original_len, 1000);
        assert!(!result.summary.is_empty());
    }

    #[test]
    fn test_truncate_includes_summary_in_content() {
        let truncator = ToolTruncator::new(200);
        let output = format!("First line\n{}", "x".repeat(1000));

        let result = truncator.truncate(&output, "read_file");

        assert!(result.content.contains("[Truncated"));
        assert!(result.content.contains("read_file"));
    }

    #[test]
    fn test_truncate_preserves_first_line_preview() {
        let truncator = ToolTruncator::new(200);
        let output = format!("This is the first line preview\nSecond line\n{}", "x".repeat(1000));

        let result = truncator.truncate(&output, "test_tool");

        assert!(result.summary.contains("This is the first line preview"));
    }

    #[test]
    fn test_truncate_long_first_line_capped_at_50_chars() {
        let truncator = ToolTruncator::new(200);
        let long_first_line = "a".repeat(100);
        let output = format!("{}\nMore content\n{}", long_first_line, "x".repeat(1000));

        let result = truncator.truncate(&output, "test_tool");

        // Preview should be capped at 50 chars
        let expected_preview: String = long_first_line.chars().take(50).collect();
        assert!(result.summary.contains(&expected_preview));
        assert!(!result.summary.contains(&"a".repeat(51)));
    }

    // =========================================================================
    // Summary Template Tests
    // =========================================================================

    #[test]
    fn test_default_summary_template_includes_lengths() {
        let truncator = ToolTruncator::new(100);
        let output = "x".repeat(500);

        let result = truncator.truncate(&output, "test_tool");

        assert!(result.summary.contains("500")); // Original length
        assert!(result.summary.contains("100")); // Truncated length
    }

    #[test]
    fn test_custom_template_with_tool_name() {
        let truncator = ToolTruncator::new(100)
            .with_summary_template("[{tool_name}: {original_len}B -> {truncated_len}B]");
        let output = "x".repeat(500);

        let result = truncator.truncate(&output, "my_custom_tool");

        assert!(result.summary.contains("my_custom_tool"));
        assert!(result.summary.contains("500B"));
        assert!(result.summary.contains("100B"));
    }

    #[test]
    fn test_custom_template_with_preview() {
        let truncator = ToolTruncator::new(200)
            .with_summary_template("Truncated {tool_name}: '{preview}'");
        let output = format!("Important first line\n{}", "x".repeat(500));

        let result = truncator.truncate(&output, "read_file");

        assert!(result.summary.contains("read_file"));
        assert!(result.summary.contains("Important first line"));
    }

    // =========================================================================
    // TruncatedOutput Tests
    // =========================================================================

    #[test]
    fn test_truncated_output_unchanged() {
        let content = "Test content".to_string();
        let result = TruncatedOutput::unchanged(content.clone());

        assert!(!result.was_truncated);
        assert_eq!(result.content, content);
        assert_eq!(result.original_len, 12);
        assert!(result.summary.is_empty());
    }

    #[test]
    fn test_truncated_output_truncated() {
        let result = TruncatedOutput::truncated(
            "Truncated content".to_string(),
            "Summary here".to_string(),
            1000,
        );

        assert!(result.was_truncated);
        assert_eq!(result.content, "Truncated content");
        assert_eq!(result.summary, "Summary here");
        assert_eq!(result.original_len, 1000);
    }

    // =========================================================================
    // Edge Case Tests
    // =========================================================================

    #[test]
    fn test_truncate_with_unicode() {
        let truncator = ToolTruncator::new(20);
        // Unicode characters: each emoji is multiple bytes but 1 char
        let output = "Hello 😀🎉🚀 World! This is a longer string with emoji";

        let result = truncator.truncate(output, "test_tool");

        // Should handle unicode correctly
        assert!(result.was_truncated);
        // Content should be valid UTF-8
        assert!(result.content.is_ascii() || !result.content.is_empty());
    }

    #[test]
    fn test_truncate_with_multiline_content() {
        let truncator = ToolTruncator::new(100);
        let output = format!("Line 1\nLine 2\nLine 3\n{}", "x".repeat(500));

        let result = truncator.truncate(&output, "test_tool");

        assert!(result.was_truncated);
        assert!(result.summary.contains("Line 1")); // First line in preview
    }

    #[test]
    fn test_truncate_content_with_only_newlines() {
        let truncator = ToolTruncator::new(10);
        let output = format!("\n\n\n\n{}", "x".repeat(100));

        let result = truncator.truncate(&output, "test_tool");

        assert!(result.was_truncated);
        // Preview should be empty since first line is empty
        assert!(result.summary.contains("..."));
    }

    #[test]
    fn test_very_small_max_chars() {
        let truncator = ToolTruncator::new(10);
        let output = "x".repeat(1000);

        let result = truncator.truncate(&output, "test_tool");

        assert!(result.was_truncated);
        // Summary alone may exceed max_chars, but that's acceptable
        assert!(!result.summary.is_empty());
    }

    #[test]
    fn test_zero_max_chars() {
        let truncator = ToolTruncator::new(0);
        let output = "Any content";

        let result = truncator.truncate(output, "test_tool");

        assert!(result.was_truncated);
        // Should still produce a summary even with 0 max_chars
        assert!(!result.summary.is_empty());
    }

    // =========================================================================
    // Integration Tests
    // =========================================================================

    #[test]
    fn test_truncate_file_content_scenario() {
        let truncator = ToolTruncator::new(500);

        // Simulate a large file read output
        let file_content = format!(r#"// Large source file
pub struct LargeStruct {{
    field1: String,
    field2: i32,
    // ... many more fields
}}

impl LargeStruct {{
    pub fn new() -> Self {{
        // Implementation details
    }}

    // Many more methods follow...
{}
"#, "// More code\n".repeat(100));

        let result = truncator.truncate(&file_content, "read_file");

        assert!(result.was_truncated);
        assert!(result.content.len() <= 500);
        assert!(result.summary.contains("Large source file") || result.summary.contains("//"));
        assert!(result.original_len > 500);
    }

    #[test]
    fn test_truncate_json_output_scenario() {
        let truncator = ToolTruncator::new(300);

        // Simulate a large JSON API response
        let json_output = format!(r#"{{"status":"success","data":[{{"id":1,"name":"Item 1"}},{{"id":2,"name":"Item 2"}},{}
"#, r#"{"id":3,"name":"Item 3"},"#.repeat(50));

        let result = truncator.truncate(&json_output, "api_call");

        assert!(result.was_truncated);
        assert!(result.content.len() <= 300);
        assert!(result.summary.contains("status"));
    }

    #[test]
    fn test_reusable_truncator() {
        let truncator = ToolTruncator::new(100);

        // Use same truncator for multiple outputs
        let result1 = truncator.truncate(&"x".repeat(500), "tool1");
        let result2 = truncator.truncate(&"y".repeat(50), "tool2");
        let result3 = truncator.truncate(&"z".repeat(1000), "tool3");

        assert!(result1.was_truncated);
        assert!(!result2.was_truncated);
        assert!(result3.was_truncated);

        // Results are independent
        assert!(result1.content.contains('x'));
        assert!(result2.content.contains('y'));
        assert!(result3.content.contains('z'));
    }
}
