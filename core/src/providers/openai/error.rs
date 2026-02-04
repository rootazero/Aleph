//! OpenAI error handling
//!
//! Error parsing and conversion utilities for OpenAI API responses.

#![allow(dead_code)]

use crate::error::AlephError;
use tracing::error;

use super::types::ErrorResponse;

/// Parse error response and convert to AlephError
pub async fn handle_error(
    provider_name: &str,
    endpoint: &str,
    response: reqwest::Response,
) -> AlephError {
    let status = response.status();

    // Try to read the raw response body first for logging
    let body_text = response.text().await.unwrap_or_else(|_| "".to_string());

    // Log the error details
    error!(
        status = %status,
        provider = %provider_name,
        endpoint = %endpoint,
        body_preview = %body_text.chars().take(500).collect::<String>(),
        "API error response"
    );

    // Try to parse error response body
    if let Ok(error_response) = serde_json::from_str::<ErrorResponse>(&body_text) {
        let error_msg = error_response.error.message;

        return match status.as_u16() {
            401 => AlephError::authentication(
                provider_name,
                &format!("Invalid API key for {}: {}", provider_name, error_msg),
            ),
            429 => AlephError::rate_limit(format!("{} rate limit: {}", provider_name, error_msg)),
            500..=599 => AlephError::provider(format!(
                "{} server error ({}): {}",
                provider_name, status, error_msg
            )),
            _ => AlephError::provider(format!(
                "{} API error ({}): {}",
                provider_name, status, error_msg
            )),
        };
    }

    // Fallback if we can't parse the error response
    match status.as_u16() {
        401 => AlephError::authentication(
            provider_name,
            &format!("Invalid API key for {}", provider_name),
        ),
        429 => AlephError::rate_limit(format!("{} rate limit exceeded", provider_name)),
        500..=599 => AlephError::provider(format!("{} server error: {}", provider_name, status)),
        _ => AlephError::provider(format!(
            "{} API error ({}): {}",
            provider_name,
            status,
            body_text.chars().take(200).collect::<String>()
        )),
    }
}

/// Maximum retry attempts for server errors (5xx)
pub const MAX_RETRIES: u32 = 3;

/// Check if a status code is retryable (5xx server errors)
pub fn is_retryable_status(status: reqwest::StatusCode) -> bool {
    status.is_server_error() // 500-599
}
