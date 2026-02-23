//! Request and response types for Google Veo API

use serde::{Deserialize, Serialize};

// === Request Types ===

/// Instance containing the prompt and optional image
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VeoInstance {
    /// The text prompt for video generation
    #[serde(skip_serializing_if = "Option::is_none")]
    pub prompt: Option<String>,

    /// Negative prompt (content to avoid)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub negative_prompt: Option<String>,

    /// Optional input image for image-to-video
    #[serde(skip_serializing_if = "Option::is_none")]
    pub image: Option<VeoImage>,
}

/// Image input for image-to-video generation
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VeoImage {
    /// Base64-encoded image bytes
    pub bytes_base64_encoded: String,
    /// MIME type of the image
    pub mime_type: String,
}

/// Parameters for video generation
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VeoParameters {
    /// Aspect ratio (16:9 or 9:16)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub aspect_ratio: Option<String>,

    /// Video duration in seconds
    #[serde(skip_serializing_if = "Option::is_none")]
    pub duration_seconds: Option<u32>,

    /// Resolution (720p, 1080p, 4k) - Veo 3 only
    #[serde(skip_serializing_if = "Option::is_none")]
    pub resolution: Option<String>,

    /// Person generation setting
    #[serde(skip_serializing_if = "Option::is_none")]
    pub person_generation: Option<String>,

    /// Whether to generate audio - Veo 3 only
    #[serde(skip_serializing_if = "Option::is_none")]
    pub generate_audio: Option<bool>,

    /// Number of videos to generate (1-4)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sample_count: Option<u32>,

    /// Random seed for reproducibility
    #[serde(skip_serializing_if = "Option::is_none")]
    pub seed: Option<u32>,

    /// Enhance prompt (Veo 2 only)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub enhance_prompt: Option<bool>,
}

/// Request body for Google Veo API
#[derive(Debug, Clone, Serialize)]
pub struct VeoRequest {
    /// Array of instances (prompts)
    pub instances: Vec<VeoInstance>,
    /// Generation parameters
    pub parameters: VeoParameters,
}

// === Response Types ===

/// Response from predictLongRunning - returns operation object
#[derive(Debug, Clone, Deserialize)]
pub struct VeoPredictResponse {
    /// Operation name for polling
    pub name: String,
}

/// Operation status response
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VeoOperationResponse {
    /// Operation name
    pub name: Option<String>,

    /// Whether operation is complete
    pub done: Option<bool>,

    /// Error if operation failed
    pub error: Option<VeoOperationError>,

    /// Response when operation is complete
    pub response: Option<VeoGenerateResponse>,

    /// Metadata about the operation
    pub metadata: Option<serde_json::Value>,
}

/// Error in operation
#[derive(Debug, Clone, Deserialize)]
pub struct VeoOperationError {
    pub code: Option<u16>,
    pub message: Option<String>,
}

/// Generated video response
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VeoGenerateResponse {
    /// Generated video samples
    pub generated_samples: Option<Vec<VeoGeneratedSample>>,
}

/// Individual generated video sample
#[derive(Debug, Clone, Deserialize)]
pub struct VeoGeneratedSample {
    /// Video data
    pub video: Option<VeoVideo>,
}

/// Video data
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VeoVideo {
    /// URI to download the video
    pub uri: Option<String>,
    /// Base64-encoded video bytes (if not using URI)
    pub bytes_base64_encoded: Option<String>,
    /// MIME type
    pub mime_type: Option<String>,
}

// === Error Types ===

/// Google API error response format
#[derive(Debug, Clone, Deserialize)]
pub struct GoogleErrorResponse {
    pub error: GoogleError,
}

/// Google API error details
#[derive(Debug, Clone, Deserialize)]
pub struct GoogleError {
    pub code: Option<i32>,
    pub message: Option<String>,
    #[allow(dead_code)] // Deserialized from API response
    pub status: Option<String>,
}
