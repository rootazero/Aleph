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
    /// Reference image for image-to-image (Volcengine Ark unified
    /// `/images/generations` endpoint: a URL or `data:`/base64 string).
    /// OpenAI-style providers use the multipart `/images/edits` endpoint
    /// instead and never set this.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub image: Option<String>,
    /// Image size (e.g., "1024x1024"). OpenAI/Ark convention.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub size: Option<String>,
    /// Image size for SiliconFlow, which names this field
    /// `image_size` instead of `size`. Only one of the two is ever set.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub image_size: Option<String>,
    /// Quality level ("standard" or "hd")
    #[serde(skip_serializing_if = "Option::is_none")]
    pub quality: Option<String>,
    /// Style ("vivid" or "natural")
    #[serde(skip_serializing_if = "Option::is_none")]
    pub style: Option<String>,
    /// Number of images to generate
    #[serde(skip_serializing_if = "Option::is_none")]
    pub n: Option<u32>,
    /// Response format ("url" or "`b64_json`")
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
    #[serde(default)]
    #[allow(dead_code)] // deserialized from API response
    pub created: u64,
    /// Array of generated images. SiliconFlow returns its results
    /// under `images` rather than OpenAI's `data`, so both keys map here.
    #[serde(alias = "images")]
    pub data: Vec<ImageData>,
}

/// Individual image data in the response
#[derive(Debug, Clone, Deserialize)]
pub struct ImageData {
    /// URL to the generated image (if `response_format` is "url")
    pub url: Option<String>,
    /// Base64-encoded image data (if `response_format` is "`b64_json`")
    pub b64_json: Option<String>,
    /// The prompt that was actually used (may differ from input)
    pub revised_prompt: Option<String>,
}

/// Polling interval for async task APIs (seconds)
pub const POLL_INTERVAL_SECS: u64 = 3;

/// Maximum polling attempts (10 minutes / 3 seconds = 200)
pub const MAX_POLL_ATTEMPTS: u32 = 200;

/// Async task submit response (e.g., video generation APIs that return a `task_id`)
///
/// Field name varies by vendor: generic proxies use `task_id`, while the
/// Volcengine Ark video task API (`/contents/generations/tasks`) returns `id`.
#[derive(Debug, Clone, Deserialize)]
pub struct AsyncTaskSubmitResponse {
    /// Task ID for polling (accepts `id` as used by Volcengine Ark)
    #[serde(alias = "id")]
    pub task_id: String,
}

/// Async task poll response
#[derive(Debug, Clone, Deserialize)]
pub struct AsyncTaskPollResponse {
    /// Task status. Generic proxies use `NOT_START`/`IN_PROGRESS`/`SUCCESS`/
    /// `FAILURE`; Volcengine Ark uses `queued`/`running`/`succeeded`/`failed`/
    /// `cancelled` (matched case-insensitively downstream).
    #[serde(default)]
    pub status: String,
    /// Failure reason (generic proxies)
    pub fail_reason: Option<String>,
    /// Structured error object (Volcengine Ark returns `{code, message}`)
    pub error: Option<TaskError>,
    /// Progress (e.g., "50%")
    pub progress: Option<String>,
    /// Result data. Volcengine Ark nests the result under `content`, so that
    /// key is aliased onto the same field.
    #[serde(alias = "content")]
    pub data: Option<AsyncTaskData>,
}

/// Structured task error (Volcengine Ark failure payload)
#[derive(Debug, Clone, Deserialize)]
pub struct TaskError {
    /// Human-readable error message
    #[serde(default)]
    pub message: String,
    /// Machine-readable error code
    #[serde(default)]
    #[allow(dead_code)] // deserialized from API response; surfaced via message
    pub code: Option<String>,
}

/// Async task result data
#[derive(Debug, Clone, Deserialize)]
pub struct AsyncTaskData {
    /// Output URL (video/audio/image)
    pub output: Option<String>,
    /// Alternative: URL field
    pub url: Option<String>,
    /// Volcengine Ark video result URL (under `content.video_url`)
    pub video_url: Option<String>,
}

impl AsyncTaskData {
    /// Get the output URL from whichever field is present
    pub fn output_url(&self) -> Option<&str> {
        self.output
            .as_deref()
            .or(self.url.as_deref())
            .or(self.video_url.as_deref())
    }
}

/// Request body for async video task APIs.
///
/// Two vendor conventions are unified into one body so a single request works
/// with both backends behind a generic OpenAI-compatible proxy:
/// - **Volcengine Ark** (`/contents/generations/tasks`) reads the `content`
///   array of typed parts (a text prompt plus optional reference images).
/// - **OpenAI-style video proxies** (e.g. T8star `/v2/videos/generations`)
///   require a top-level `prompt` string and ignore `content`.
///
/// Both fields are always sent: `prompt` mirrors the text of the first
/// `content` part, so each backend reads the field it understands and ignores
/// the other.
#[derive(Debug, Clone, Serialize)]
pub struct VideoTaskRequest {
    /// Model to use (e.g., "doubao-seedance-2-0-260128")
    pub model: String,
    /// Top-level prompt for OpenAI-style video proxies that require it.
    /// Mirrors the first `content` text part (with Seedance `--flag` suffixes).
    pub prompt: String,
    /// Ordered content parts (text prompt + optional reference images),
    /// consumed by Volcengine Ark.
    pub content: Vec<VideoContentPart>,
}

/// A single content part in a video task request.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type")]
pub enum VideoContentPart {
    /// Text prompt (Seedance encodes params as `--flag value` suffixes)
    #[serde(rename = "text")]
    Text { text: String },
    /// Reference image (image-to-video / first-frame control)
    #[serde(rename = "image_url")]
    ImageUrl { image_url: ImageUrlRef },
}

/// Image reference wrapper (`{ "url": "..." }`).
#[derive(Debug, Clone, Serialize)]
pub struct ImageUrlRef {
    /// HTTP(S) URL or `data:` URL of the reference image
    pub url: String,
}

/// `OpenAI` API error response format
#[derive(Debug, Clone, Deserialize)]
pub struct OpenAiErrorResponse {
    pub error: OpenAiError,
}

/// `OpenAI` API error details
#[derive(Debug, Clone, Deserialize)]
pub struct OpenAiError {
    pub message: String,
    #[serde(rename = "type")]
    pub error_type: String,
    #[serde(default)]
    #[allow(dead_code)] // Deserialized from API response
    pub param: Option<String>,
    #[serde(default)]
    #[allow(dead_code)] // Deserialized from API response
    pub code: Option<String>,
}
