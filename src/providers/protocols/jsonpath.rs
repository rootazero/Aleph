//! JSONPath parser for extracting values from provider responses
//!
//! Different AI providers return responses in different JSON structures.
//! This module provides JSONPath querying to extract values from arbitrary
//! JSON responses using expressions like `$.data.choices[0].message.content`.

use crate::error::{AlephError, Result};
use jsonpath_rust::JsonPath;
use serde_json::Value;

/// Extract a value from JSON using a JSONPath expression
///
/// # Arguments
///
/// * `json` - The JSON value to query
/// * `path` - JSONPath expression (e.g., "$.data.choices[0].message.content")
///
/// # Returns
///
/// The first matching value as a String. Different value types are handled:
/// - String: returned as-is
/// - Number: converted to string representation
/// - Bool: converted to "true" or "false"
/// - Object/Array: serialized to JSON string
/// - Null: returns "null" (only if the path exists and has null value)
///
/// # Errors
///
/// Returns `AlephError::ProviderError` if:
/// - The JSONPath expression is invalid
/// - The path does not exist in the JSON structure
/// - No values match the path
/// - JSON serialization fails
///
/// # Example
///
/// ```rust,ignore
/// use serde_json::json;
/// use alephcore::providers::protocols::extract_value;
///
/// let json = json!({
///     "data": {
///         "choices": [
///             {"message": {"content": "Hello, world!"}}
///         ]
///     }
/// });
///
/// let result = extract_value(&json, "$.data.choices[0].message.content")?;
/// assert_eq!(result, "Hello, world!");
/// ```
pub fn extract_value(json: &Value, path: &str) -> Result<String> {
    // Execute the JSONPath query (jsonpath-rust 1.0 `JsonPath::query` returns the
    // matched values in document order, borrowing from the input JSON).
    let results = json
        .query(path)
        .map_err(|e| AlephError::provider(format!("JSONPath query failed: {e}")))?;

    // RFC 9535 semantics make path existence unambiguous: an empty result set
    // means the path matched nothing, while a present `Value::Null` means the
    // path exists and holds a JSON null. We extract the first match.
    let first_match = match results.first() {
        Some(&value) => value,
        None => {
            return Err(AlephError::provider(format!(
                "No value found at JSONPath '{path}' in response (path does not exist)"
            )));
        }
    };

    // Convert the Value to String based on type
    let value_str = match first_match {
        Value::String(s) => s.clone(),
        Value::Number(n) => n.to_string(),
        Value::Bool(b) => b.to_string(),
        Value::Null => "null".to_string(),
        Value::Object(_) | Value::Array(_) => {
            // Serialize complex types to JSON string
            serde_json::to_string(first_match).map_err(|e| {
                AlephError::provider(format!("Failed to serialize JSON value: {e}"))
            })?
        }
    };

    Ok(value_str)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_extract_simple_string() {
        let json = json!({
            "message": "Hello, world!"
        });

        let result = extract_value(&json, "$.message").unwrap();
        assert_eq!(result, "Hello, world!");
    }

    #[test]
    fn test_extract_nested_value() {
        let json = json!({
            "data": {
                "choices": [
                    {
                        "message": {
                            "content": "AI response here"
                        }
                    }
                ]
            }
        });

        let result = extract_value(&json, "$.data.choices[0].message.content").unwrap();
        assert_eq!(result, "AI response here");
    }

    #[test]
    fn test_extract_nonexistent_path() {
        let json = json!({
            "message": "Hello"
        });

        // Nonexistent paths should return an error
        let result = extract_value(&json, "$.nonexistent.path");
        assert!(result.is_err());

        if let Err(AlephError::ProviderError { message, .. }) = result {
            assert!(
                message.contains("No value found at JSONPath")
                    && message.contains("path does not exist")
            );
        } else {
            panic!(
                "Expected ProviderError for nonexistent path, got: {:?}",
                result
            );
        }
    }

    #[test]
    fn test_extract_number() {
        let json = json!({
            "count": 42
        });

        let result = extract_value(&json, "$.count").unwrap();
        assert_eq!(result, "42");
    }

    #[test]
    fn test_extract_bool() {
        let json = json!({
            "success": true
        });

        let result = extract_value(&json, "$.success").unwrap();
        assert_eq!(result, "true");
    }

    #[test]
    fn test_extract_actual_null() {
        // Test extracting a field that exists but has null value
        // This should succeed and return "null" string
        let json = json!({
            "value": null
        });

        let result = extract_value(&json, "$.value").unwrap();
        assert_eq!(result, "null");
    }

    #[test]
    fn test_extract_nested_nonexistent_path() {
        // Test that nested nonexistent paths also error correctly
        let json = json!({
            "data": {
                "message": "Hello"
            }
        });

        let result = extract_value(&json, "$.data.nonexistent.field");
        assert!(result.is_err());

        if let Err(AlephError::ProviderError { message, .. }) = result {
            assert!(message.contains("path does not exist"));
        } else {
            panic!("Expected ProviderError for nested nonexistent path");
        }
    }

    #[test]
    fn test_extract_object() {
        let json = json!({
            "metadata": {
                "model": "gpt-4",
                "tokens": 100
            }
        });

        let result = extract_value(&json, "$.metadata").unwrap();
        let parsed: Value = serde_json::from_str(&result).unwrap();
        assert_eq!(parsed["model"], "gpt-4");
        assert_eq!(parsed["tokens"], 100);
    }

    #[test]
    fn test_extract_array() {
        let json = json!({
            "items": ["a", "b", "c"]
        });

        let result = extract_value(&json, "$.items").unwrap();
        let parsed: Value = serde_json::from_str(&result).unwrap();
        assert_eq!(parsed, json!(["a", "b", "c"]));
    }

    #[test]
    fn test_invalid_jsonpath_expression() {
        let json = json!({"data": "test"});

        // Invalid JSONPath syntax
        let result = extract_value(&json, "$.[invalid");
        assert!(result.is_err());

        if let Err(AlephError::ProviderError { message, .. }) = result {
            assert!(
                message.contains("JSONPath query failed")
                    || message.contains("Invalid JSONPath expression")
            );
        } else {
            panic!(
                "Expected ProviderError for invalid JSONPath, got: {:?}",
                result
            );
        }
    }

    #[test]
    fn test_array_first_match() {
        // JSONPath can return multiple matches, we should get the first one
        let json = json!({
            "choices": [
                {"text": "first"},
                {"text": "second"}
            ]
        });

        let result = extract_value(&json, "$.choices[*].text").unwrap();
        assert_eq!(result, "first");
    }

    #[test]
    fn test_real_openai_response() {
        let json = json!({
            "id": "chatcmpl-123",
            "object": "chat.completion",
            "choices": [
                {
                    "index": 0,
                    "message": {
                        "role": "assistant",
                        "content": "This is the AI response"
                    },
                    "finish_reason": "stop"
                }
            ]
        });

        let result = extract_value(&json, "$.choices[0].message.content").unwrap();
        assert_eq!(result, "This is the AI response");
    }

    #[test]
    fn test_real_anthropic_response() {
        let json = json!({
            "id": "msg_123",
            "type": "message",
            "content": [
                {
                    "type": "text",
                    "text": "Claude's response here"
                }
            ]
        });

        let result = extract_value(&json, "$.content[0].text").unwrap();
        assert_eq!(result, "Claude's response here");
    }
}
