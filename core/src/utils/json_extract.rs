//! Robust JSON extraction utilities for AI response parsing
//!
//! This module provides reliable JSON extraction from AI responses that may contain:
//! - Pure JSON responses
//! - JSON embedded in markdown code blocks
//! - JSON mixed with explanatory text
//! - Multiple JSON objects (extracts the first complete one)
//!
//! The extraction uses proper brace-matching instead of greedy search to handle
//! nested JSON structures and embedded braces in strings correctly.

use tracing::debug;

/// Extract the first complete JSON object from a response string.
///
/// Tries multiple strategies in order:
/// 1. Direct JSON parse (response is pure JSON)
/// 2. Extract from ```json code block
/// 3. Extract from generic ``` code block
/// 4. Find first complete JSON object using brace matching
///
/// # Arguments
///
/// * `response` - The raw response string that may contain JSON
///
/// # Returns
///
/// * `Some(Value)` - Successfully extracted and parsed JSON
/// * `None` - No valid JSON found
///
/// # Examples
///
/// ```
/// use alephcore::utils::json_extract::extract_json_robust;
///
/// // Pure JSON
/// let result = extract_json_robust(r#"{"tool": "search"}"#);
/// assert!(result.is_some());
///
/// // JSON in markdown code block
/// let result = extract_json_robust("```json\n{\"tool\": \"search\"}\n```");
/// assert!(result.is_some());
///
/// // JSON with surrounding text
/// let result = extract_json_robust("Result: {\"tool\": \"search\"} done.");
/// assert!(result.is_some());
/// ```
pub fn extract_json_robust(response: &str) -> Option<serde_json::Value> {
    let response = response.trim();

    if response.is_empty() {
        return None;
    }

    // Strategy 1: Direct parse (response is pure JSON)
    if let Ok(v) = serde_json::from_str(response) {
        debug!(strategy = "direct", "JSON extraction successful");
        return Some(v);
    }

    // Strategy 2: Extract from ```json code block
    if let Some(json_str) = extract_from_json_code_block(response) {
        if let Ok(v) = serde_json::from_str(&json_str) {
            debug!(strategy = "json_code_block", "JSON extraction successful");
            return Some(v);
        }
    }

    // Strategy 3: Extract from generic ``` code block
    if let Some(json_str) = extract_from_generic_code_block(response) {
        if let Ok(v) = serde_json::from_str(&json_str) {
            debug!(
                strategy = "generic_code_block",
                "JSON extraction successful"
            );
            return Some(v);
        }
    }

    // Strategy 4: Find first complete JSON object using brace matching
    if let Some(start) = response.find('{') {
        if let Some(end) = find_matching_brace(response, start) {
            let candidate = &response[start..=end];
            if let Ok(v) = serde_json::from_str(candidate) {
                debug!(strategy = "brace_match", "JSON extraction successful");
                return Some(v);
            }
        }
    }

    debug!("JSON extraction failed: no valid JSON found");
    None
}

/// Extract JSON string from a ```json code block
fn extract_from_json_code_block(response: &str) -> Option<String> {
    let start_marker = "```json";
    let end_marker = "```";

    if let Some(start) = response.find(start_marker) {
        let json_start = start + start_marker.len();
        // Skip any whitespace/newlines after ```json
        let content = &response[json_start..];
        let content_start = content
            .find(|c: char| !c.is_whitespace() || c == '{')
            .unwrap_or(0);

        if let Some(end) = content[content_start..].find(end_marker) {
            return Some(
                content[content_start..content_start + end]
                    .trim()
                    .to_string(),
            );
        }
    }
    None
}

/// Extract JSON string from a generic ``` code block
fn extract_from_generic_code_block(response: &str) -> Option<String> {
    let marker = "```";

    if let Some(start) = response.find(marker) {
        let block_start = start + marker.len();
        // Skip language identifier if present (find first newline)
        let content_start = response[block_start..]
            .find('\n')
            .map(|i| block_start + i + 1)
            .unwrap_or(block_start);

        if let Some(end) = response[content_start..].find(marker) {
            let content = &response[content_start..content_start + end];
            let trimmed = content.trim();
            // Only return if it looks like JSON
            if trimmed.starts_with('{') {
                return Some(trimmed.to_string());
            }
        }
    }
    None
}

/// Find the index of the closing brace that matches the opening brace at `start`
///
/// This function correctly handles:
/// - Nested braces
/// - Braces inside JSON strings (ignored)
/// - Escaped characters inside strings
///
/// # Arguments
///
/// * `s` - The string to search
/// * `start` - The index of the opening brace '{'
///
/// # Returns
///
/// * `Some(index)` - The index of the matching closing brace '}'
/// * `None` - No matching brace found
fn find_matching_brace(s: &str, start: usize) -> Option<usize> {
    let mut depth = 0;
    let mut in_string = false;
    let mut escape_next = false;

    for (i, ch) in s[start..].char_indices() {
        if escape_next {
            escape_next = false;
            continue;
        }

        match ch {
            '\\' if in_string => {
                escape_next = true;
            }
            '"' => {
                in_string = !in_string;
            }
            '{' if !in_string => {
                depth += 1;
            }
            '}' if !in_string => {
                depth -= 1;
                if depth == 0 {
                    return Some(start + i);
                }
            }
            _ => {}
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_pure_json() {
        let input = r#"{"tool": "search", "confidence": 0.9}"#;
        let result = extract_json_robust(input);
        assert!(result.is_some());
        let json = result.unwrap();
        assert_eq!(json["tool"], "search");
        assert_eq!(json["confidence"], 0.9);
    }

    #[test]
    fn test_extract_from_json_code_block() {
        let input = r#"Here is the result:

```json
{
  "tool": "search",
  "confidence": 0.8
}
```

That's the output."#;

        let result = extract_json_robust(input);
        assert!(result.is_some());
        let json = result.unwrap();
        assert_eq!(json["tool"], "search");
        assert_eq!(json["confidence"], 0.8);
    }

    #[test]
    fn test_extract_from_generic_code_block() {
        let input = r#"
```
{"tool": "video", "confidence": 0.7}
```
"#;

        let result = extract_json_robust(input);
        assert!(result.is_some());
        let json = result.unwrap();
        assert_eq!(json["tool"], "video");
    }

    #[test]
    fn test_extract_first_json_from_multiple_objects() {
        // This is the key test case that the old greedy approach failed
        let input = r#"Result: {"tool": "a"} and {"tool": "b"}"#;
        let result = extract_json_robust(input);
        assert!(result.is_some());
        let json = result.unwrap();
        // Should extract the FIRST complete JSON object
        assert_eq!(json["tool"], "a");
    }

    #[test]
    fn test_extract_nested_json() {
        let input = r#"{"outer": {"inner": {"deep": 1}}}"#;
        let result = extract_json_robust(input);
        assert!(result.is_some());
        let json = result.unwrap();
        assert_eq!(json["outer"]["inner"]["deep"], 1);
    }

    #[test]
    fn test_extract_json_with_embedded_braces_in_strings() {
        let input = r#"{"text": "This has } and { braces", "valid": true}"#;
        let result = extract_json_robust(input);
        assert!(result.is_some());
        let json = result.unwrap();
        assert_eq!(json["text"], "This has } and { braces");
        assert_eq!(json["valid"], true);
    }

    #[test]
    fn test_extract_json_with_escaped_quotes() {
        let input = r#"{"message": "He said \"hello\"", "count": 1}"#;
        let result = extract_json_robust(input);
        assert!(result.is_some());
        let json = result.unwrap();
        assert_eq!(json["message"], r#"He said "hello""#);
    }

    #[test]
    fn test_extract_json_with_surrounding_text() {
        let input = "The analysis shows: {\"confidence\": 0.95} as expected.";
        let result = extract_json_robust(input);
        assert!(result.is_some());
        let json = result.unwrap();
        assert_eq!(json["confidence"], 0.95);
    }

    #[test]
    fn test_return_none_for_invalid_json() {
        let input = "This is not JSON at all";
        let result = extract_json_robust(input);
        assert!(result.is_none());
    }

    #[test]
    fn test_return_none_for_malformed_json() {
        let input = r#"{"tool": "search", confidence: 0.9}"#; // Missing quotes
        let result = extract_json_robust(input);
        assert!(result.is_none());
    }

    #[test]
    fn test_return_none_for_empty_input() {
        let result = extract_json_robust("");
        assert!(result.is_none());

        let result = extract_json_robust("   ");
        assert!(result.is_none());
    }

    #[test]
    fn test_extract_json_array() {
        let input = r#"[{"name": "a"}, {"name": "b"}]"#;
        let result = extract_json_robust(input);
        assert!(result.is_some());
        let json = result.unwrap();
        assert!(json.is_array());
    }

    #[test]
    fn test_extract_complex_nested_structure() {
        let input = r#"{
            "tool": "search",
            "parameters": {
                "query": "test",
                "filters": {
                    "date": "2024",
                    "tags": ["ai", "ml"]
                }
            },
            "metadata": {
                "source": "user"
            }
        }"#;

        let result = extract_json_robust(input);
        assert!(result.is_some());
        let json = result.unwrap();
        assert_eq!(json["tool"], "search");
        assert_eq!(json["parameters"]["query"], "test");
        assert_eq!(json["parameters"]["filters"]["tags"][0], "ai");
    }

    #[test]
    fn test_find_matching_brace_simple() {
        let s = "{}";
        assert_eq!(find_matching_brace(s, 0), Some(1));
    }

    #[test]
    fn test_find_matching_brace_nested() {
        let s = "{{{}}}";
        assert_eq!(find_matching_brace(s, 0), Some(5));
    }

    #[test]
    fn test_find_matching_brace_with_string() {
        let s = r#"{"text": "}"}"#;
        assert_eq!(find_matching_brace(s, 0), Some(12));
    }

    #[test]
    fn test_find_matching_brace_no_match() {
        let s = "{";
        assert_eq!(find_matching_brace(s, 0), None);
    }
}
