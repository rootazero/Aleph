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
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GeminiFunctionCall {
    pub name: String,
    pub args: Value,
    /// Native tool call ID (Gemini 3+ models). Absent on older models.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
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
    InlineData {
        #[serde(rename = "inlineData")]
        inline_data: InlineData,
    },
    /// Function call from model
    FunctionCall {
        #[serde(rename = "functionCall")]
        function_call: GeminiFunctionCall,
        /// Gemini 3 `thoughtSignature` — a Part-level sibling of `functionCall`
        /// (NOT a member of the `functionCall` object). It is an opaque token
        /// the model requires replayed verbatim on later turns. Omitted from
        /// the wire when absent (older models, non-Gemini origin).
        #[serde(
            rename = "thoughtSignature",
            default,
            skip_serializing_if = "Option::is_none"
        )]
        thought_signature: Option<String>,
    },
    /// Function response from user
    FunctionResponse {
        #[serde(rename = "functionResponse")]
        function_response: GeminiFunctionResponse,
    },
}

/// Function response structure
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GeminiFunctionResponse {
    pub name: String,
    pub response: serde_json::Value,
    /// Pass back the tool call ID when available
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
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
    /// Stop sequences — generation halts when any of these strings is produced.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stop_sequences: Option<Vec<String>>,
    /// Media resolution for image inputs
    /// (`MEDIA_RESOLUTION_LOW` / `MEDIUM` / `HIGH`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub media_resolution: Option<String>,
}

/// Thinking configuration for Gemini.
///
/// - Gemini 2.5 models use `thinking_budget` (integer token count, -1=dynamic)
/// - Gemini 3+ models use `thinking_level` (enum: MINIMAL/LOW/MEDIUM/HIGH)
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ThinkingConfig {
    /// Token budget for thinking (Gemini 2.5). -1 = dynamic.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thinking_budget: Option<i32>,
    /// Thinking level enum (Gemini 3+): "MINIMAL", "LOW", "MEDIUM", "HIGH"
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thinking_level: Option<String>,
    /// Whether to include thought content in response
    #[serde(skip_serializing_if = "Option::is_none")]
    pub include_thoughts: Option<bool>,
}

/// Error returned by the Gemini API (the `error` field of the JSON envelope).
///
/// The streaming-first request path parses live candidate responses straight
/// from the raw SSE JSON in `sse.rs`; only the error envelope still needs a
/// typed shape.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GeminiError {
    pub code: i32,
    pub message: String,
    pub status: String,
    /// Structured error details (`google.rpc.RetryInfo`, `ErrorInfo`, ...).
    #[serde(default)]
    pub details: Vec<Value>,
}

impl GeminiError {
    /// Extract a retry delay (seconds, as a display string) from a
    /// `google.rpc.RetryInfo` entry in `details`. Gemini puts the
    /// authoritative 429 backoff here, often without a `Retry-After` header.
    pub fn retry_delay_secs(&self) -> Option<String> {
        for detail in &self.details {
            let is_retry_info = detail
                .get("@type")
                .and_then(|t| t.as_str())
                .is_some_and(|t| t.ends_with("RetryInfo"));
            if !is_retry_info {
                continue;
            }
            if let Some(delay) = detail.get("retryDelay").and_then(|d| d.as_str()) {
                // protobuf Duration string, e.g. "30s" / "1.5s"
                let trimmed = delay.trim_end_matches('s').trim();
                if !trimmed.is_empty() {
                    return Some(trimmed.to_string());
                }
            }
        }
        None
    }
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
                stop_sequences: None,
                media_resolution: None,
            }),
            tools: None,
        };

        let json = serde_json::to_string(&request).unwrap();
        assert!(json.contains("contents"));
        assert!(json.contains("generationConfig"));
        assert!(!json.contains("tools")); // None should be skipped
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
                thinking_level: None,
                include_thoughts: None,
            }),
            stop_sequences: None,
            media_resolution: None,
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
    fn test_thinking_config_level_mode() {
        let config = GenerationConfig {
            max_output_tokens: None,
            temperature: None,
            top_p: None,
            top_k: None,
            thinking_config: Some(ThinkingConfig {
                thinking_budget: None,
                thinking_level: Some("HIGH".to_string()),
                include_thoughts: Some(true),
            }),
            stop_sequences: None,
            media_resolution: None,
        };
        let json = serde_json::to_string(&config).unwrap();
        assert!(json.contains("thinkingLevel"));
        assert!(json.contains("HIGH"));
        assert!(json.contains("includeThoughts"));
        assert!(!json.contains("thinkingBudget"));
    }

    #[test]
    fn test_generation_config_serializes_stop_and_media() {
        let config = GenerationConfig {
            max_output_tokens: None,
            temperature: None,
            top_p: None,
            top_k: None,
            thinking_config: None,
            stop_sequences: Some(vec!["STOP".to_string(), "END".to_string()]),
            media_resolution: Some("MEDIA_RESOLUTION_LOW".to_string()),
        };
        let json = serde_json::to_string(&config).unwrap();
        assert!(json.contains("stopSequences"));
        assert!(json.contains("mediaResolution"));
        assert!(json.contains("MEDIA_RESOLUTION_LOW"));
    }

    #[test]
    fn test_gemini_error_retry_delay_from_details() {
        let err: GeminiError = serde_json::from_str(
            r#"{
                "code": 429,
                "message": "Resource exhausted",
                "status": "RESOURCE_EXHAUSTED",
                "details": [
                    {"@type": "type.googleapis.com/google.rpc.RetryInfo", "retryDelay": "42s"}
                ]
            }"#,
        )
        .unwrap();
        assert_eq!(err.retry_delay_secs().as_deref(), Some("42"));
    }

    #[test]
    fn test_gemini_error_retry_delay_absent() {
        let err: GeminiError = serde_json::from_str(
            r#"{"code": 400, "message": "bad", "status": "INVALID_ARGUMENT"}"#,
        )
        .unwrap();
        assert_eq!(err.retry_delay_secs(), None);
    }
}
