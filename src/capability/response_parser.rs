//! Response parser for AI-first intent detection.
//!
//! This module parses AI responses to detect capability invocation requests
//! and clarification needs. It handles various response formats including
//! JSON in markdown code blocks.

use super::request::{AiResponse, RawCapabilityRequest, RawClarificationRequest};
use crate::error::Result;
use tracing::debug;

/// Parser for AI responses that may contain capability requests or clarification needs.
pub struct ResponseParser;

impl ResponseParser {
    /// Parse an AI response to detect capability requests or clarification needs.
    ///
    /// The AI may respond in several formats:
    /// 1. Direct text (no capability or clarification needed)
    /// 2. JSON with `__capability_request__: true` marker
    /// 3. JSON with `__needs_clarification__: true` marker
    /// 4. JSON wrapped in markdown code block
    ///
    /// # Arguments
    ///
    /// * `response` - The raw AI response text
    ///
    /// # Returns
    ///
    /// An `AiResponse` indicating direct response, capability request, or clarification need.
    pub fn parse(response: &str) -> Result<AiResponse> {
        let response = response.trim();

        // Try to extract and parse JSON
        if let Some(json_str) = Self::extract_json(response) {
            // Try capability request first
            if let Ok(raw) = serde_json::from_str::<RawCapabilityRequest>(&json_str) {
                if raw.is_capability_request {
                    debug!(
                        capability = %raw.capability,
                        query = %raw.query,
                        "Detected capability request in AI response"
                    );
                    return Ok(AiResponse::CapabilityRequest(raw.into()));
                }
            }

            // Try clarification request
            if let Ok(raw) = serde_json::from_str::<RawClarificationRequest>(&json_str) {
                if raw.needs_clarification {
                    debug!(
                        reason = %raw.reason,
                        prompt = %raw.prompt,
                        "Detected clarification request in AI response"
                    );
                    return Ok(AiResponse::NeedsClarification(raw.into()));
                }
            }
        }

        // Not a capability or clarification request - return as direct response
        Ok(AiResponse::Direct(response.to_string()))
    }

    /// Parse response specifically for clarification signals.
    ///
    /// This is a convenience method for checking if a response needs clarification
    /// without fully parsing capability requests.
    pub fn parse_clarification(response: &str) -> Option<super::request::ClarificationInfo> {
        let response = response.trim();

        if let Some(json_str) = Self::extract_json(response) {
            if let Ok(raw) = serde_json::from_str::<RawClarificationRequest>(&json_str) {
                if raw.needs_clarification {
                    return Some(raw.into());
                }
            }
        }

        None
    }

    /// Extract JSON from various response formats.
    ///
    /// Handles:
    /// - Plain JSON starting with `{`
    /// - JSON in markdown code blocks (```json ... ```)
    /// - JSON embedded in text
    fn extract_json(response: &str) -> Option<String> {
        // Try markdown code block first
        if let Some(json) = Self::extract_from_code_block(response) {
            return Some(json);
        }

        // Try plain JSON
        if response.starts_with('{') {
            if let Some(end) = Self::find_matching_brace(response) {
                return Some(response[..=end].to_string());
            }
        }

        // Try to find JSON embedded in text
        if let Some(start) = response.find('{') {
            if let Some(end) = Self::find_matching_brace(&response[start..]) {
                return Some(response[start..start + end + 1].to_string());
            }
        }

        None
    }

    /// Extract JSON from a markdown code block.
    fn extract_from_code_block(response: &str) -> Option<String> {
        // Look for ```json or ``` followed by {
        let patterns = ["```json\n", "```json\r\n", "```\n{", "```\r\n{"];

        for pattern in patterns {
            if let Some(start) = response.find(pattern) {
                let json_start = if pattern.ends_with('{') {
                    start + pattern.len() - 1 // Keep the opening brace
                } else {
                    start + pattern.len()
                };

                // Find the closing ```
                if let Some(end) = response[json_start..].find("```") {
                    let json = response[json_start..json_start + end].trim();
                    return Some(json.to_string());
                }
            }
        }

        None
    }

    /// Find the byte position of the matching closing brace for a JSON object.
    ///
    /// Returns the byte offset (not character index) for proper UTF-8 string slicing.
    fn find_matching_brace(s: &str) -> Option<usize> {
        if !s.starts_with('{') {
            return None;
        }

        let mut depth = 0;
        let mut in_string = false;
        let mut escape_next = false;

        // Use char_indices() to get byte offsets for correct UTF-8 slicing
        for (byte_pos, c) in s.char_indices() {
            if escape_next {
                escape_next = false;
                continue;
            }

            if c == '\\' && in_string {
                escape_next = true;
                continue;
            }

            if c == '"' {
                in_string = !in_string;
                continue;
            }

            if in_string {
                continue;
            }

            match c {
                '{' => depth += 1,
                '}' => {
                    depth -= 1;
                    if depth == 0 {
                        return Some(byte_pos);
                    }
                }
                _ => {}
            }
        }

        None
    }

    /// Check if a response looks like it might contain a capability request.
    ///
    /// This is a quick pre-check before full parsing.
    pub fn might_be_capability_request(response: &str) -> bool {
        response.contains("__capability_request__") || response.contains("\"capability\":")
    }

    /// Check if a response looks like it might need clarification.
    ///
    /// This is a quick pre-check before full parsing.
    pub fn might_need_clarification(response: &str) -> bool {
        response.contains("__needs_clarification__")
    }

    /// Check if a response contains any special markers (capability or clarification).
    pub fn has_special_markers(response: &str) -> bool {
        Self::might_be_capability_request(response) || Self::might_need_clarification(response)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_direct_response() {
        let response = "Hello, how can I help you today?";
        let result = ResponseParser::parse(response).unwrap();
        assert!(result.is_direct());
        assert_eq!(result.as_direct().unwrap(), response);
    }

    #[test]
    fn test_parse_plain_json_capability_request() {
        let response = r#"{"__capability_request__": true, "capability": "search", "parameters": {"query": "weather"}, "query": "What's the weather?"}"#;
        let result = ResponseParser::parse(response).unwrap();
        assert!(result.is_capability_request());
        let req = result.as_capability_request().unwrap();
        assert_eq!(req.capability, "search");
        assert_eq!(req.query, "What's the weather?");
    }

    #[test]
    fn test_parse_markdown_code_block() {
        let response = r#"I'll search for that information.

```json
{"__capability_request__": true, "capability": "search", "parameters": {"query": "Tokyo weather"}, "query": "weather in Tokyo"}
```"#;

        let result = ResponseParser::parse(response).unwrap();
        assert!(result.is_capability_request());
        let req = result.as_capability_request().unwrap();
        assert_eq!(req.capability, "search");
    }

    #[test]
    fn test_parse_embedded_json() {
        let response = r#"Let me search for that: {"__capability_request__": true, "capability": "video", "parameters": {"url": "https://youtube.com/watch?v=xxx"}, "query": "summarize video"}"#;

        let result = ResponseParser::parse(response).unwrap();
        assert!(result.is_capability_request());
        let req = result.as_capability_request().unwrap();
        assert_eq!(req.capability, "video");
    }

    #[test]
    fn test_parse_json_without_marker() {
        // JSON without __capability_request__ should be treated as direct response
        let response = r#"{"message": "hello"}"#;
        let result = ResponseParser::parse(response).unwrap();
        assert!(result.is_direct());
    }

    #[test]
    fn test_parse_json_with_false_marker() {
        // JSON with __capability_request__: false should be treated as direct
        let response = r#"{"__capability_request__": false, "capability": "search"}"#;
        let result = ResponseParser::parse(response).unwrap();
        assert!(result.is_direct());
    }

    #[test]
    fn test_find_matching_brace_simple() {
        assert_eq!(ResponseParser::find_matching_brace("{}"), Some(1));
        assert_eq!(ResponseParser::find_matching_brace("{a}"), Some(2));
    }

    #[test]
    fn test_find_matching_brace_nested() {
        assert_eq!(ResponseParser::find_matching_brace("{{}}"), Some(3));
        assert_eq!(
            ResponseParser::find_matching_brace(r#"{"a": {"b": 1}}"#),
            Some(14)
        );
    }

    #[test]
    fn test_find_matching_brace_with_strings() {
        // Braces inside strings should be ignored
        assert_eq!(
            ResponseParser::find_matching_brace(r#"{"a": "}"}"#),
            Some(9)
        );
    }

    #[test]
    fn test_find_matching_brace_with_escaped_quotes() {
        assert_eq!(
            ResponseParser::find_matching_brace(r#"{"a": "\"}"}"#),
            Some(11)
        );
    }

    #[test]
    fn test_might_be_capability_request() {
        assert!(ResponseParser::might_be_capability_request(
            r#"{"__capability_request__": true}"#
        ));
        assert!(ResponseParser::might_be_capability_request(
            r#"{"capability": "search"}"#
        ));
        assert!(!ResponseParser::might_be_capability_request("Hello world"));
    }

    #[test]
    fn test_parse_with_reasoning() {
        let response = r#"{"__capability_request__": true, "capability": "search", "parameters": {"query": "北京天气"}, "query": "今天天气怎么样", "reasoning": "用户询问天气信息，需要实时数据"}"#;

        let result = ResponseParser::parse(response).unwrap();
        assert!(result.is_capability_request());
        let req = result.as_capability_request().unwrap();
        assert!(req.reasoning.is_some());
        assert!(req.reasoning.as_ref().unwrap().contains("实时数据"));
    }

    // ========================================
    // Clarification Parsing Tests
    // ========================================

    #[test]
    fn test_parse_clarification_request() {
        let response = r#"{"__needs_clarification__": true, "reason": "ambiguous", "prompt": "Which city do you mean?"}"#;

        let result = ResponseParser::parse(response).unwrap();
        assert!(result.needs_user_clarification());

        let info = result.as_clarification().unwrap();
        assert_eq!(info.prompt, "Which city do you mean?");
    }

    #[test]
    fn test_parse_clarification_with_suggestions() {
        let response = r#"{
            "__needs_clarification__": true,
            "reason": "ambiguous",
            "prompt": "您是想了解北京还是上海的天气？",
            "context_summary": "用户询问天气但未指定城市",
            "suggestions": ["北京", "上海", "广州"]
        }"#;

        let result = ResponseParser::parse(response).unwrap();
        assert!(result.needs_user_clarification());

        let info = result.as_clarification().unwrap();
        assert!(info.has_suggestions());
        assert_eq!(info.suggestions.as_ref().unwrap().len(), 3);
    }

    #[test]
    fn test_parse_clarification_in_code_block() {
        let response = r#"I need more information.

```json
{"__needs_clarification__": true, "reason": "missing_parameter", "prompt": "What language?"}
```"#;

        let result = ResponseParser::parse(response).unwrap();
        assert!(result.needs_user_clarification());
    }

    #[test]
    fn test_parse_clarification_helper() {
        let response = r#"{"__needs_clarification__": true, "reason": "insufficient_info", "prompt": "Please specify"}"#;

        let info = ResponseParser::parse_clarification(response);
        assert!(info.is_some());

        let info = info.unwrap();
        assert_eq!(info.prompt, "Please specify");
    }

    #[test]
    fn test_might_need_clarification() {
        assert!(ResponseParser::might_need_clarification(
            r#"{"__needs_clarification__": true}"#
        ));
        assert!(!ResponseParser::might_need_clarification("Hello world"));
    }

    #[test]
    fn test_has_special_markers() {
        assert!(ResponseParser::has_special_markers(
            r#"{"__capability_request__": true}"#
        ));
        assert!(ResponseParser::has_special_markers(
            r#"{"__needs_clarification__": true}"#
        ));
        assert!(!ResponseParser::has_special_markers("Plain text response"));
    }

    #[test]
    fn test_capability_takes_priority_over_clarification() {
        // If both markers are present (unlikely but test edge case),
        // capability request should be detected first
        let response = r#"{
            "__capability_request__": true,
            "__needs_clarification__": false,
            "capability": "search",
            "parameters": {},
            "query": "test"
        }"#;

        let result = ResponseParser::parse(response).unwrap();
        assert!(result.is_capability_request());
        assert!(!result.needs_user_clarification());
    }
}
