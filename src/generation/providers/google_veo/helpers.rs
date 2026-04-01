//! Helper functions for Google Veo Provider

use super::constants::*;
use super::types::GoogleErrorResponse;
use crate::generation::GenerationError;

/// Check if an aspect ratio is valid for Veo
pub fn is_valid_aspect_ratio(ratio: &str) -> bool {
    ASPECT_RATIOS.contains(&ratio)
}

/// Check if a resolution is valid for Veo 3
pub fn is_valid_resolution(resolution: &str) -> bool {
    RESOLUTIONS.contains(&resolution)
}

/// Check if a duration is valid for Veo 3
pub fn is_valid_veo3_duration(duration: u32) -> bool {
    VEO3_DURATIONS.contains(&duration)
}

/// Check if a duration is valid for Veo 2
pub fn is_valid_veo2_duration(duration: u32) -> bool {
    duration >= VEO2_DURATION_RANGE.0 && duration <= VEO2_DURATION_RANGE.1
}

/// Parse API error response and convert to GenerationError
pub fn parse_error_response(status: reqwest::StatusCode, body: &str) -> GenerationError {
    // Try to parse as Google API error format
    if let Ok(error_response) = serde_json::from_str::<GoogleErrorResponse>(body) {
        let message = error_response
            .error
            .message
            .unwrap_or_else(|| "Unknown error".to_string());
        let code = error_response.error.code;

        // Check for specific error types
        if message.to_lowercase().contains("safety") || message.contains("blocked") {
            return GenerationError::content_filtered(message, Some("safety".to_string()));
        }
        if message.contains("quota") || message.contains("limit") {
            return GenerationError::quota_exceeded(message, None);
        }

        return GenerationError::provider(message, code.map(|c| c as u16), "google-veo");
    }

    // Handle based on status code
    match status.as_u16() {
        400 => GenerationError::invalid_parameters(body.to_string(), None),
        401 | 403 => {
            GenerationError::authentication("Invalid API key or unauthorized", "google-veo")
        }
        429 => GenerationError::rate_limit("Rate limit exceeded", None),
        404 => GenerationError::model_not_found(DEFAULT_MODEL, "google-veo"),
        500..=599 => GenerationError::provider(
            format!("Google API server error: {}", body),
            Some(status.as_u16()),
            "google-veo",
        ),
        _ => GenerationError::provider(
            format!("Unexpected error: {}", body),
            Some(status.as_u16()),
            "google-veo",
        ),
    }
}
