//! AI response suggestion parsing module.
//!
//! This module parses AI responses for follow-up suggestions and converts them
//! into clarification options for continued conversation.
//!
//! # Example
//!
//! ```ignore
//! let parser = SuggestionParser::new();
//! let result = parser.parse("这是天气信息。你还需要我帮你查看：
//! 1. 明天天气
//! 2. 穿衣建议
//! 3. 空气质量");
//!
//! if result.has_suggestions {
//!     // Present options to user via clarification
//! }
//! ```

use regex::Regex;

use crate::clarification::{ClarificationOption, ClarificationRequest};

/// A single suggestion option extracted from AI response.
#[derive(Debug, Clone)]
pub struct SuggestionOption {
    /// Display label for the option
    pub label: String,
    /// Value to use when user selects this option
    pub value: String,
}

impl SuggestionOption {
    /// Create a new suggestion option.
    pub fn new(label: impl Into<String>, value: impl Into<String>) -> Self {
        SuggestionOption {
            label: label.into(),
            value: value.into(),
        }
    }
}

/// Result of parsing AI response for suggestions.
#[derive(Debug, Clone)]
pub struct ParsedSuggestions {
    /// Whether any suggestions were found
    pub has_suggestions: bool,
    /// The response with suggestion section removed (for clean display)
    pub cleaned_response: String,
    /// Extracted suggestion options
    pub options: Vec<SuggestionOption>,
    /// Position where suggestions start in original response
    pub suggestion_start: Option<usize>,
}

impl ParsedSuggestions {
    /// Create an empty result (no suggestions found).
    pub fn empty(response: &str) -> Self {
        ParsedSuggestions {
            has_suggestions: false,
            cleaned_response: response.to_string(),
            options: Vec::new(),
            suggestion_start: None,
        }
    }

    /// Convert parsed suggestions to a clarification request.
    pub fn to_clarification_request(&self) -> Option<ClarificationRequest> {
        if !self.has_suggestions || self.options.is_empty() {
            return None;
        }

        let options: Vec<ClarificationOption> = self
            .options
            .iter()
            .map(|s| ClarificationOption::new(&s.value, &s.label))
            .collect();

        Some(
            ClarificationRequest::select("ai-follow-up", "你还需要:", options)
                .with_source("ai:suggestion"),
        )
    }
}

/// Pattern for detecting and extracting suggestions.
#[derive(Debug, Clone)]
struct SuggestionPattern {
    /// Regex to detect suggestion section
    trigger: Regex,
    /// Type of extraction to perform
    extractor: ExtractionMethod,
}

/// Method for extracting individual suggestions.
#[derive(Debug, Clone)]
enum ExtractionMethod {
    /// Extract numbered items (1. xxx 2. yyy)
    NumberedList,
    /// Extract bullet points (- xxx)
    BulletList,
    /// Extract from inline options (xxx | yyy | zzz)
    InlineOptions,
}

/// Parser for extracting suggestions from AI responses.
pub struct SuggestionParser {
    /// Patterns to detect suggestions
    patterns: Vec<SuggestionPattern>,
    /// Regex for numbered list extraction
    numbered_regex: Regex,
    /// Regex for bullet list extraction
    bullet_regex: Regex,
    /// Whether suggestion parsing is enabled
    enabled: bool,
    /// Maximum suggestions to extract
    max_suggestions: usize,
}

impl SuggestionParser {
    /// Create a new suggestion parser with default patterns.
    pub fn new() -> Self {
        SuggestionParser {
            patterns: vec![
                SuggestionPattern {
                    trigger: Regex::new(
                        r"(?i)(你还需要我帮你|还有什么需要|还需要了解|还想知道|是否需要|需要我|要我帮你)",
                    )
                    .unwrap(),
                    extractor: ExtractionMethod::NumberedList,
                },
                SuggestionPattern {
                    trigger: Regex::new(r"(?i)(以下是一些|可以选择|你可以|建议你)").unwrap(),
                    // Auto-detect: try bullet list first, then numbered
                    extractor: ExtractionMethod::BulletList,
                },
            ],
            numbered_regex: Regex::new(r"(\d+)[\.、\)]\s*([^\n\d]+)").unwrap(),
            bullet_regex: Regex::new(r"[\-\*•]\s*([^\n]+)").unwrap(),
            enabled: true,
            max_suggestions: 5,
        }
    }

    /// Enable or disable suggestion parsing.
    pub fn set_enabled(&mut self, enabled: bool) {
        self.enabled = enabled;
    }

    /// Check if suggestion parsing is enabled.
    pub fn is_enabled(&self) -> bool {
        self.enabled
    }

    /// Set maximum number of suggestions to extract.
    pub fn set_max_suggestions(&mut self, max: usize) {
        self.max_suggestions = max;
    }

    /// Parse AI response for suggestions.
    pub fn parse(&self, response: &str) -> ParsedSuggestions {
        if !self.enabled {
            return ParsedSuggestions::empty(response);
        }

        // Find the first matching pattern
        for pattern in &self.patterns {
            if let Some(m) = pattern.trigger.find(response) {
                let suggestion_start = m.start();
                let suggestion_text = &response[suggestion_start..];

                // Extract options based on method
                let options = match pattern.extractor {
                    ExtractionMethod::NumberedList => {
                        self.extract_numbered_list(suggestion_text)
                    }
                    ExtractionMethod::BulletList => self.extract_bullet_list(suggestion_text),
                    ExtractionMethod::InlineOptions => {
                        self.extract_inline_options(suggestion_text)
                    }
                };

                if !options.is_empty() {
                    // Clean the response by removing or trimming the suggestion section
                    let cleaned = self.clean_response(response, suggestion_start);

                    return ParsedSuggestions {
                        has_suggestions: true,
                        cleaned_response: cleaned,
                        options,
                        suggestion_start: Some(suggestion_start),
                    };
                }
            }
        }

        // No suggestions found
        ParsedSuggestions::empty(response)
    }

    /// Extract numbered list items.
    fn extract_numbered_list(&self, text: &str) -> Vec<SuggestionOption> {
        self.numbered_regex
            .captures_iter(text)
            .take(self.max_suggestions)
            .filter_map(|cap| {
                let content = cap.get(2)?.as_str().trim();
                if content.is_empty() {
                    return None;
                }
                // Clean up the content
                let cleaned = self.clean_option_text(content);
                if cleaned.is_empty() {
                    return None;
                }
                Some(SuggestionOption::new(&cleaned, &cleaned))
            })
            .collect()
    }

    /// Extract bullet list items.
    fn extract_bullet_list(&self, text: &str) -> Vec<SuggestionOption> {
        self.bullet_regex
            .captures_iter(text)
            .take(self.max_suggestions)
            .filter_map(|cap| {
                let content = cap.get(1)?.as_str().trim();
                if content.is_empty() {
                    return None;
                }
                let cleaned = self.clean_option_text(content);
                if cleaned.is_empty() {
                    return None;
                }
                Some(SuggestionOption::new(&cleaned, &cleaned))
            })
            .collect()
    }

    /// Extract inline options (xxx | yyy | zzz).
    fn extract_inline_options(&self, text: &str) -> Vec<SuggestionOption> {
        // Look for pattern like "A | B | C" or "A / B / C"
        let delimiter_regex = Regex::new(r"\s*[\|/]\s*").unwrap();

        // Find a line with multiple delimiters
        for line in text.lines() {
            let parts: Vec<&str> = delimiter_regex.split(line).collect();
            if parts.len() >= 2 {
                return parts
                    .iter()
                    .take(self.max_suggestions)
                    .filter_map(|p| {
                        let cleaned = self.clean_option_text(p.trim());
                        if cleaned.is_empty() || cleaned.len() > 50 {
                            return None;
                        }
                        Some(SuggestionOption::new(&cleaned, &cleaned))
                    })
                    .collect();
            }
        }

        Vec::new()
    }

    /// Clean up option text by removing common suffixes and formatting.
    fn clean_option_text(&self, text: &str) -> String {
        let mut cleaned = text.to_string();

        // Remove common suffixes
        let suffixes = ["吗", "？", "?", "。", "...", "…"];
        for suffix in suffixes {
            cleaned = cleaned.trim_end_matches(suffix).to_string();
        }

        // Remove leading/trailing punctuation
        cleaned = cleaned.trim_matches(|c: char| c.is_ascii_punctuation()).to_string();

        cleaned.trim().to_string()
    }

    /// Clean response by removing or formatting suggestion section.
    fn clean_response(&self, response: &str, suggestion_start: usize) -> String {
        // Keep content before suggestions, trim trailing whitespace
        let before = response[..suggestion_start].trim_end();
        before.to_string()
    }
}

impl Default for SuggestionParser {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_numbered_suggestions() {
        let parser = SuggestionParser::new();

        let response = "上海今天晴天，25°C。你还需要我帮你查看：
1. 明天天气
2. 穿衣建议
3. 空气质量";

        let result = parser.parse(response);

        assert!(result.has_suggestions);
        assert_eq!(result.options.len(), 3);
        assert_eq!(result.options[0].label, "明天天气");
        assert_eq!(result.options[1].label, "穿衣建议");
        assert_eq!(result.options[2].label, "空气质量");
        assert!(result.cleaned_response.contains("25°C"));
        assert!(!result.cleaned_response.contains("你还需要"));
    }

    #[test]
    fn test_parse_chinese_numbered_suggestions() {
        let parser = SuggestionParser::new();

        let response = "这是答案。还有什么需要帮忙的吗？
1、查看更多信息
2、导出报告
3、发送邮件";

        let result = parser.parse(response);

        assert!(result.has_suggestions);
        assert_eq!(result.options.len(), 3);
    }

    #[test]
    fn test_no_suggestions() {
        let parser = SuggestionParser::new();

        let response = "这是一个普通的回复，没有任何后续建议。";

        let result = parser.parse(response);

        assert!(!result.has_suggestions);
        assert!(result.options.is_empty());
        assert_eq!(result.cleaned_response, response);
    }

    #[test]
    fn test_parse_disabled() {
        let mut parser = SuggestionParser::new();
        parser.set_enabled(false);

        let response = "你还需要我帮你：1. 选项A 2. 选项B";

        let result = parser.parse(response);

        assert!(!result.has_suggestions);
    }

    #[test]
    fn test_max_suggestions() {
        let mut parser = SuggestionParser::new();
        parser.set_max_suggestions(2);

        let response = "你还需要我帮你：
1. 选项A
2. 选项B
3. 选项C
4. 选项D";

        let result = parser.parse(response);

        assert!(result.has_suggestions);
        assert_eq!(result.options.len(), 2);
    }

    #[test]
    fn test_to_clarification_request() {
        let parser = SuggestionParser::new();

        let response = "回答。你还需要我帮你：1. 选项A 2. 选项B";
        let result = parser.parse(response);

        let request = result.to_clarification_request();
        assert!(request.is_some());

        let req = request.unwrap();
        assert_eq!(req.id, "ai-follow-up");
        assert_eq!(req.source, Some("ai:suggestion".to_string()));
    }

    #[test]
    fn test_clean_option_text() {
        let parser = SuggestionParser::new();

        assert_eq!(parser.clean_option_text("选项A？"), "选项A");
        assert_eq!(parser.clean_option_text("选项B吗"), "选项B");
        assert_eq!(parser.clean_option_text("选项C。"), "选项C");
    }

    #[test]
    fn test_bullet_list_extraction() {
        let parser = SuggestionParser::new();

        let response = "你可以选择以下选项：
- 导出为PDF
- 发送邮件
- 打印文档";

        let result = parser.parse(response);

        assert!(result.has_suggestions);
        assert_eq!(result.options.len(), 3);
    }
}
