//! Google Gemini API types
//!
//! Type definitions for the Google Gemini API protocol.
//! Based on: https://ai.google.dev/api/rest/v1beta/models/generateContent

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Request body for Gemini generateContent API
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GenerateContentRequest {
    pub contents: Vec<Content>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub system_instruction: Option<Content>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub generation_config: Option<GenerationConfig>,
    /// Tool configurations containing function declarations
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tools: Option<Vec<GeminiToolConfig>>,
}

// =============================================================================
// Tool types for Gemini function calling
// =============================================================================

/// Tool configuration for Gemini API
///
/// Each tool config contains a list of function declarations that
/// the model may call during generation.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GeminiToolConfig {
    pub function_declarations: Vec<GeminiFunctionDeclaration>,
}

/// A single function declaration for Gemini
///
/// Describes a tool the model can invoke, with a JSON Schema
/// for its parameters.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GeminiFunctionDeclaration {
    pub name: String,
    pub description: String,
    pub parameters: Value,
}

/// Function call in a Gemini response part
///
/// Gemini does not assign a unique ID to function calls, so
/// the caller must generate synthetic IDs.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GeminiFunctionCall {
    pub name: String,
    pub args: Value,
}

/// Content structure for Gemini API (messages)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Content {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub role: Option<String>,
    pub parts: Vec<Part>,
}

/// Part can be text or inline image data
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Part {
    /// Text content part
    Text { text: String },
    /// Inline image data part
    InlineData { inline_data: InlineData },
}

/// Inline data for images
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InlineData {
    /// MIME type (e.g., "image/png", "image/jpeg")
    pub mime_type: String,
    /// Base64-encoded image data (without data URI prefix)
    pub data: String,
}

/// Generation configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GenerationConfig {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_output_tokens: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub top_p: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub top_k: Option<u32>,
    /// Extended thinking configuration (Gemini experimental)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thinking_config: Option<ThinkingConfig>,
}

/// Thinking configuration for Gemini (experimental feature)
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ThinkingConfig {
    /// Budget for thinking tokens
    pub thinking_budget: Option<u32>,
}

/// Response from Gemini generateContent API
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GenerateContentResponse {
    pub candidates: Option<Vec<Candidate>>,
    pub error: Option<GeminiError>,
}

/// Candidate response
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Candidate {
    pub content: CandidateContent,
    /// Why the model stopped generating (e.g., "STOP", "FUNCTION_CALL", "MAX_TOKENS")
    pub finish_reason: Option<String>,
}

/// Content in candidate response
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CandidateContent {
    pub parts: Vec<ResponsePart>,
}

/// Response part — may contain text, a function call, or both
///
/// Gemini response parts use a flat JSON object with optional keys:
/// `{"text": "..."}` or `{"functionCall": {"name": "...", "args": {...}}}`
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ResponsePart {
    /// Text content (present for text parts)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    /// Function call (present for tool-use parts)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub function_call: Option<GeminiFunctionCall>,
}

/// Error response from Gemini API
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GeminiError {
    pub code: i32,
    pub message: String,
    pub status: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_serialize_request() {
        let request = GenerateContentRequest {
            contents: vec![Content {
                role: Some("user".to_string()),
                parts: vec![Part::Text {
                    text: "Hello".to_string(),
                }],
            }],
            system_instruction: None,
            generation_config: Some(GenerationConfig {
                max_output_tokens: Some(1024),
                temperature: Some(0.7),
                top_p: None,
                top_k: None,
                thinking_config: None,
            }),
            tools: None,
        };

        let json = serde_json::to_string(&request).unwrap();
        assert!(json.contains("contents"));
        assert!(json.contains("generationConfig"));
        assert!(!json.contains("tools")); // None should be skipped
    }

    #[test]
    fn test_deserialize_response() {
        let json = r#"{
            "candidates": [{
                "content": {
                    "parts": [{"text": "Hello!"}]
                }
            }]
        }"#;

        let response: GenerateContentResponse = serde_json::from_str(json).unwrap();
        assert!(response.candidates.is_some());
        let candidates = response.candidates.unwrap();
        let text = candidates[0].content.parts[0].text.as_deref();
        assert_eq!(text, Some("Hello!"));
    }

    #[test]
    fn test_deserialize_error() {
        let json = r#"{
            "error": {
                "code": 400,
                "message": "Invalid request",
                "status": "INVALID_ARGUMENT"
            }
        }"#;

        let response: GenerateContentResponse = serde_json::from_str(json).unwrap();
        assert!(response.error.is_some());
        assert_eq!(response.error.unwrap().code, 400);
    }

    #[test]
    fn test_thinking_config() {
        let config = GenerationConfig {
            max_output_tokens: Some(2048),
            temperature: Some(0.9),
            top_p: Some(0.95),
            top_k: Some(40),
            thinking_config: Some(ThinkingConfig {
                thinking_budget: Some(1000),
            }),
        };

        let json = serde_json::to_string(&config).unwrap();
        assert!(json.contains("thinkingConfig"));
        assert!(json.contains("thinkingBudget"));
    }

    #[test]
    fn test_serialize_tool_config() {
        let tool_config = GeminiToolConfig {
            function_declarations: vec![GeminiFunctionDeclaration {
                name: "search".to_string(),
                description: "Search the web".to_string(),
                parameters: serde_json::json!({
                    "type": "object",
                    "properties": {
                        "query": {"type": "string"}
                    },
                    "required": ["query"]
                }),
            }],
        };

        let json = serde_json::to_string(&tool_config).unwrap();
        assert!(json.contains("functionDeclarations"));
        assert!(json.contains("search"));
        assert!(json.contains("Search the web"));
    }

    #[test]
    fn test_serialize_request_with_tools() {
        let request = GenerateContentRequest {
            contents: vec![Content {
                role: Some("user".to_string()),
                parts: vec![Part::Text {
                    text: "Search for Rust".to_string(),
                }],
            }],
            system_instruction: None,
            generation_config: None,
            tools: Some(vec![GeminiToolConfig {
                function_declarations: vec![GeminiFunctionDeclaration {
                    name: "search".to_string(),
                    description: "Search the web".to_string(),
                    parameters: serde_json::json!({"type": "object"}),
                }],
            }]),
        };

        let json = serde_json::to_string(&request).unwrap();
        assert!(json.contains("tools"));
        assert!(json.contains("functionDeclarations"));
    }

    #[test]
    fn test_deserialize_response_with_function_call() {
        let json = r#"{
            "candidates": [{
                "content": {
                    "parts": [{
                        "functionCall": {
                            "name": "search",
                            "args": {"query": "Rust language"}
                        }
                    }]
                },
                "finishReason": "FUNCTION_CALL"
            }]
        }"#;

        let response: GenerateContentResponse = serde_json::from_str(json).unwrap();
        let candidates = response.candidates.unwrap();
        let part = &candidates[0].content.parts[0];

        assert!(part.text.is_none());
        let fc = part.function_call.as_ref().unwrap();
        assert_eq!(fc.name, "search");
        assert_eq!(fc.args["query"], "Rust language");
        assert_eq!(candidates[0].finish_reason.as_deref(), Some("FUNCTION_CALL"));
    }

    #[test]
    fn test_deserialize_response_with_mixed_parts() {
        let json = r#"{
            "candidates": [{
                "content": {
                    "parts": [
                        {"text": "Let me search for that"},
                        {"functionCall": {"name": "search", "args": {"q": "test"}}}
                    ]
                },
                "finishReason": "FUNCTION_CALL"
            }]
        }"#;

        let response: GenerateContentResponse = serde_json::from_str(json).unwrap();
        let parts = &response.candidates.unwrap()[0].content.parts;

        assert_eq!(parts.len(), 2);
        assert_eq!(parts[0].text.as_deref(), Some("Let me search for that"));
        assert!(parts[0].function_call.is_none());
        assert!(parts[1].text.is_none());
        assert_eq!(parts[1].function_call.as_ref().unwrap().name, "search");
    }

    #[test]
    fn test_deserialize_response_with_finish_reason() {
        let json = r#"{
            "candidates": [{
                "content": {
                    "parts": [{"text": "Done"}]
                },
                "finishReason": "STOP"
            }]
        }"#;

        let response: GenerateContentResponse = serde_json::from_str(json).unwrap();
        let candidate = &response.candidates.unwrap()[0];
        assert_eq!(candidate.finish_reason.as_deref(), Some("STOP"));
    }
}
