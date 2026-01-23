//! Error handling for Replicate API responses
//!
//! This module provides error parsing and conversion utilities.

use super::types::ErrorResponse;
use crate::generation::GenerationError;

/// Parse API error response into GenerationError
pub fn parse_error_response(status: u16, body: &str) -> GenerationError {
    // Try to parse as JSON error
    if let Ok(error_response) = serde_json::from_str::<ErrorResponse>(body) {
        let message = error_response
            .detail
            .unwrap_or_else(|| error_response.title.unwrap_or_else(|| body.to_string()));

        // Check for specific error types
        if message.to_lowercase().contains("rate limit") {
            return GenerationError::rate_limit(message, None);
        }
        if message.to_lowercase().contains("unauthorized")
            || message.to_lowercase().contains("invalid token")
        {
            return GenerationError::authentication(message, "replicate");
        }
    }

    // Handle based on status code
    match status {
        401 => GenerationError::authentication("Invalid API token", "replicate"),
        402 => GenerationError::quota_exceeded("Payment required or credits exhausted", None),
        403 => GenerationError::authentication("Access forbidden", "replicate"),
        404 => GenerationError::model_not_found("Model or prediction not found", "replicate"),
        422 => GenerationError::invalid_parameters(body.to_string(), None),
        429 => GenerationError::rate_limit("Rate limit exceeded", None),
        500..=599 => GenerationError::provider(
            format!("Server error: {}", body),
            Some(status),
            "replicate",
        ),
        _ => GenerationError::provider(
            format!("Unexpected error: {}", body),
            Some(status),
            "replicate",
        ),
    }
}
