//! Helper functions for OpenAI-compatible provider
//!
//! Contains utility functions for error parsing and request building.

use crate::generation::{GenerationError, GenerationRequest};

use super::provider::OpenAiCompatProvider;
use super::types::{ImageGenerationRequest, OpenAiErrorResponse};

impl OpenAiCompatProvider {
    /// Get the full URL for the images/generations endpoint
    pub(crate) fn generations_url(&self) -> String {
        format!("{}/v1/images/generations", self.endpoint)
    }

    /// Get the full URL for the images/edits endpoint
    pub(crate) fn edits_url(&self) -> String {
        format!("{}/v1/images/edits", self.endpoint)
    }

    /// Build the API request body from a GenerationRequest
    pub(crate) fn build_request_body(&self, request: &GenerationRequest) -> ImageGenerationRequest {
        let model = request
            .params
            .model
            .clone()
            .unwrap_or_else(|| self.model.clone());

        // Build size string from width/height if provided
        let size = match (request.params.width, request.params.height) {
            (Some(w), Some(h)) => Some(format!("{}x{}", w, h)),
            _ => None,
        };

        ImageGenerationRequest {
            model,
            prompt: request.prompt.clone(),
            size,
            quality: request.params.quality.clone(),
            style: request.params.style.clone(),
            n: request.params.n,
            response_format: Some("url".to_string()), // Default to URL format
            user: request.user_id.clone(),
        }
    }

    /// Parse API error response and convert to GenerationError
    pub(crate) fn parse_error_response(
        &self,
        status: reqwest::StatusCode,
        body: &str,
    ) -> GenerationError {
        // Try to parse as OpenAI error format
        if let Ok(error_response) = serde_json::from_str::<OpenAiErrorResponse>(body) {
            let message = error_response.error.message;
            let error_type = error_response.error.error_type;

            // Check for specific error types
            if error_type == "invalid_request_error" {
                // Check for content policy violations
                if message.contains("content policy")
                    || message.contains("safety system")
                    || message.contains("prohibited")
                {
                    return GenerationError::content_filtered(message, None);
                }
                return GenerationError::invalid_parameters(message, None);
            }
        }

        // Handle based on status code
        match status.as_u16() {
            401 => GenerationError::authentication("Invalid API key or unauthorized", &self.name),
            429 => {
                // Try to extract retry-after from response
                GenerationError::rate_limit("Rate limit exceeded", None)
            }
            400 => GenerationError::invalid_parameters(body.to_string(), None),
            403 => GenerationError::authentication(
                "Access forbidden - check your API key permissions",
                &self.name,
            ),
            404 => GenerationError::model_not_found(&self.model, &self.name),
            500..=599 => GenerationError::provider(
                format!("Server error: {}", body),
                Some(status.as_u16()),
                &self.name,
            ),
            _ => GenerationError::provider(
                format!("Unexpected error: {}", body),
                Some(status.as_u16()),
                &self.name,
            ),
        }
    }
}
