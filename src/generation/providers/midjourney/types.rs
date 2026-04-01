//! Type definitions for Midjourney API
//!
//! Contains all request/response types and enums used by the Midjourney provider.

use serde::{Deserialize, Serialize};

// === Constants ===

/// Default API endpoint for T8Star Midjourney service
pub const DEFAULT_ENDPOINT: &str = "https://ai.t8star.cn";

/// Default timeout for HTTP requests (30 seconds per request, not total)
pub const DEFAULT_REQUEST_TIMEOUT_SECS: u64 = 30;

/// Polling interval in seconds
pub const POLL_INTERVAL_SECS: u64 = 1;

/// Maximum number of poll attempts (300 * 1s = 5 minutes)
pub const MAX_POLL_ATTEMPTS: u32 = 300;

/// Provider name for identification
pub const PROVIDER_NAME: &str = "midjourney";

/// Midjourney brand color (Discord-esque blue)
pub const DEFAULT_COLOR: &str = "#5865F2";

// === Enums ===

/// Midjourney generation mode
///
/// Controls the priority and speed of image generation.
///
/// # Modes
///
/// - `Fast` - Higher priority queue, faster generation times
/// - `Relax` - Lower priority queue, more cost-effective
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum MidjourneyMode {
    /// Fast mode - higher priority, faster generation
    #[default]
    Fast,
    /// Relax mode - lower priority, cost-effective
    Relax,
}

impl MidjourneyMode {
    /// Get the API path prefix for this mode
    pub fn as_path(&self) -> &'static str {
        match self {
            MidjourneyMode::Fast => "mj-fast",
            MidjourneyMode::Relax => "mj-relax",
        }
    }
}

impl std::fmt::Display for MidjourneyMode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            MidjourneyMode::Fast => write!(f, "fast"),
            MidjourneyMode::Relax => write!(f, "relax"),
        }
    }
}

// === API Request/Response Types ===

/// Request body for Midjourney imagine endpoint
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ImagineRequest {
    /// The text prompt for image generation
    pub prompt: String,

    /// Optional base64-encoded images for reference
    #[serde(skip_serializing_if = "Option::is_none")]
    pub base64_array: Option<Vec<String>>,
}

/// Response from submit endpoint
#[derive(Debug, Clone, Deserialize)]
pub struct SubmitResponse {
    /// Response code (1 = success)
    pub code: i32,

    /// Human-readable description
    pub description: Option<String>,

    /// Task ID on success
    pub result: Option<String>,
}

/// Response from task fetch endpoint
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskResponse {
    /// Task ID
    pub id: String,

    /// Task status (NOT_START, SUBMITTED, IN_PROGRESS, SUCCESS, FAILURE)
    pub status: String,

    /// Progress percentage (e.g., "50%")
    pub progress: Option<String>,

    /// Generated image URL on success
    pub image_url: Option<String>,

    /// Failure reason if task failed
    pub fail_reason: Option<String>,

    /// Action buttons for variations/upscales
    pub buttons: Option<Vec<TaskButton>>,
}

/// Action button for task actions
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskButton {
    /// Custom ID for the action
    pub custom_id: String,

    /// Button label (e.g., "U1", "V1", "Vary (Region)")
    pub label: String,
}
