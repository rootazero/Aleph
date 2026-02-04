//! Google Gemini API types
//!
//! Type definitions for the Google Gemini API protocol.
//! Based on: https://ai.google.dev/api/rest/v1beta/models/generateContent

use serde::{Deserialize, Serialize};

/// Request body for Gemini generateContent API
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GenerateContentRequest {
    pub contents: Vec<Content>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub system_instruction: Option<Content>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub generation_config: Option<GenerationConfig>,
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
pub struct Candidate {
    pub content: CandidateContent,
}

/// Content in candidate response
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CandidateContent {
    pub parts: Vec<ResponsePart>,
}

/// Response part with text
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResponsePart {
    pub text: String,
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
        };

        let json = serde_json::to_string(&request).unwrap();
        assert!(json.contains("contents"));
        assert!(json.contains("generationConfig"));
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
        let text = &response.candidates.unwrap()[0].content.parts[0].text;
        assert_eq!(text, "Hello!");
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
}
