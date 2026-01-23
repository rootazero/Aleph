//! Type definitions for OpenAI-compatible API
//!
//! Contains request/response structures and constants for the OpenAI-compatible
//! image generation API.

use serde::{Deserialize, Serialize};

/// Default model for image generation
pub const DEFAULT_MODEL: &str = "dall-e-3";

/// Default timeout for image generation requests (120 seconds)
pub const DEFAULT_TIMEOUT_SECS: u64 = 120;

/// Default brand color
pub const DEFAULT_COLOR: &str = "#6366f1"; // Indigo

/// Request body for OpenAI-compatible image generation API
#[derive(Debug, Clone, Serialize)]
pub struct ImageGenerationRequest {
    /// Model to use (e.g., "dall-e-3")
    pub model: String,
    /// The prompt to generate an image from
    pub prompt: String,
    /// Image size (e.g., "1024x1024")
    #[serde(skip_serializing_if = "Option::is_none")]
    pub size: Option<String>,
    /// Quality level ("standard" or "hd")
    #[serde(skip_serializing_if = "Option::is_none")]
    pub quality: Option<String>,
    /// Style ("vivid" or "natural")
    #[serde(skip_serializing_if = "Option::is_none")]
    pub style: Option<String>,
    /// Number of images to generate
    #[serde(skip_serializing_if = "Option::is_none")]
    pub n: Option<u32>,
    /// Response format ("url" or "b64_json")
    #[serde(skip_serializing_if = "Option::is_none")]
    pub response_format: Option<String>,
    /// Optional user identifier
    #[serde(skip_serializing_if = "Option::is_none")]
    pub user: Option<String>,
}

/// Response from OpenAI-compatible image generation API
#[derive(Debug, Clone, Deserialize)]
pub struct ImageGenerationResponse {
    /// Unix timestamp of when the request was created
    #[allow(dead_code)]
    pub created: u64,
    /// Array of generated images
    pub data: Vec<ImageData>,
}

/// Individual image data in the response
#[derive(Debug, Clone, Deserialize)]
pub struct ImageData {
    /// URL to the generated image (if response_format is "url")
    pub url: Option<String>,
    /// Base64-encoded image data (if response_format is "b64_json")
    pub b64_json: Option<String>,
    /// The prompt that was actually used (may differ from input)
    pub revised_prompt: Option<String>,
}

/// OpenAI API error response format
#[derive(Debug, Clone, Deserialize)]
pub struct OpenAiErrorResponse {
    pub error: OpenAiError,
}

/// OpenAI API error details
#[derive(Debug, Clone, Deserialize)]
pub struct OpenAiError {
    pub message: String,
    #[serde(rename = "type")]
    pub error_type: String,
    #[serde(default)]
    #[allow(dead_code)]
    pub param: Option<String>,
    #[serde(default)]
    #[allow(dead_code)]
    pub code: Option<String>,
}
