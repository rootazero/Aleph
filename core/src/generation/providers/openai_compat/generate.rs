//! Image generation implementation for OpenAI-compatible provider
//!
//! Contains the `GenerationProvider::generate` implementation.

use base64::Engine;
use std::future::Future;
use std::pin::Pin;
use std::time::{Duration, Instant};
use tracing::{debug, error, info};

use crate::generation::{
    GenerationData, GenerationError, GenerationMetadata, GenerationOutput, GenerationProvider,
    GenerationRequest, GenerationResult, GenerationType,
};

use super::provider::OpenAiCompatProvider;
use super::types::{ImageGenerationResponse, DEFAULT_TIMEOUT_SECS};

impl GenerationProvider for OpenAiCompatProvider {
    fn generate(
        &self,
        request: GenerationRequest,
    ) -> Pin<Box<dyn Future<Output = GenerationResult<GenerationOutput>> + Send + '_>> {
        Box::pin(async move {
            // Validate generation type
            if !self.supported_types.contains(&request.generation_type) {
                return Err(GenerationError::unsupported_generation_type(
                    request.generation_type.to_string(),
                    &self.name,
                ));
            }

            let start_time = Instant::now();
            let request_id = request.request_id.clone();

            debug!(
                provider = %self.name,
                prompt = %request.prompt,
                model = %self.model,
                "Starting OpenAI-compatible image generation"
            );

            // Build request body
            let body = self.build_request_body(&request);
            let url = self.generations_url();

            debug!(url = %url, "Sending request to OpenAI-compatible API");

            // Make API request
            let response = self
                .client
                .post(&url)
                .header("Authorization", format!("Bearer {}", self.api_key))
                .header("Content-Type", "application/json")
                .json(&body)
                .send()
                .await
                .map_err(|e| {
                    if e.is_timeout() {
                        GenerationError::timeout(Duration::from_secs(DEFAULT_TIMEOUT_SECS))
                    } else if e.is_connect() {
                        GenerationError::network(format!("Connection failed: {}", e))
                    } else {
                        GenerationError::network(e.to_string())
                    }
                })?;

            let status = response.status();
            let response_text = response.text().await.map_err(|e| {
                GenerationError::network(format!("Failed to read response body: {}", e))
            })?;

            // Handle non-success status codes
            if !status.is_success() {
                error!(
                    provider = %self.name,
                    status = %status,
                    body = %response_text,
                    "OpenAI-compatible API request failed"
                );
                return Err(self.parse_error_response(status, &response_text));
            }

            // Parse successful response
            let api_response: ImageGenerationResponse = serde_json::from_str(&response_text)
                .map_err(|e| {
                    error!(
                        error = %e,
                        body = %response_text,
                        "Failed to parse OpenAI-compatible response"
                    );
                    GenerationError::serialization(format!("Failed to parse response: {}", e))
                })?;

            // Extract first image
            let first_image = api_response.data.first().ok_or_else(|| {
                GenerationError::provider("No images in response", None, &self.name)
            })?;

            // Convert to GenerationData
            let data = if let Some(url) = &first_image.url {
                GenerationData::url(url.clone())
            } else if let Some(b64) = &first_image.b64_json {
                // Decode base64 to bytes
                let bytes = base64::engine::general_purpose::STANDARD
                    .decode(b64)
                    .map_err(|e| {
                        GenerationError::serialization(format!("Failed to decode base64: {}", e))
                    })?;
                GenerationData::bytes(bytes)
            } else {
                return Err(GenerationError::provider(
                    "Response contains neither URL nor base64 data",
                    None,
                    &self.name,
                ));
            };

            // Build metadata
            let duration = start_time.elapsed();
            let mut metadata = GenerationMetadata::new()
                .with_provider(&self.name)
                .with_model(body.model.clone())
                .with_duration(duration);

            if let Some(revised) = &first_image.revised_prompt {
                metadata = metadata.with_revised_prompt(revised.clone());
            }

            // Add dimensions from request params
            if let (Some(w), Some(h)) = (request.params.width, request.params.height) {
                metadata = metadata.with_dimensions(w, h);
            }

            info!(
                provider = %self.name,
                duration_ms = duration.as_millis(),
                model = %body.model,
                "OpenAI-compatible image generation completed"
            );

            // Build output
            let mut output =
                GenerationOutput::new(request.generation_type, data).with_metadata(metadata);

            if let Some(id) = request_id {
                output = output.with_request_id(id);
            }

            // Handle additional images (if n > 1 and provider supports it)
            if api_response.data.len() > 1 {
                let additional: Vec<GenerationData> = api_response
                    .data
                    .iter()
                    .skip(1)
                    .filter_map(|img| {
                        if let Some(url) = &img.url {
                            Some(GenerationData::url(url.clone()))
                        } else if let Some(b64) = &img.b64_json {
                            base64::engine::general_purpose::STANDARD
                                .decode(b64)
                                .ok()
                                .map(GenerationData::bytes)
                        } else {
                            None
                        }
                    })
                    .collect();

                if !additional.is_empty() {
                    output = output.with_additional_outputs(additional);
                }
            }

            Ok(output)
        })
    }

    fn name(&self) -> &str {
        &self.name
    }

    fn supported_types(&self) -> Vec<GenerationType> {
        self.supported_types.clone()
    }

    fn color(&self) -> &str {
        &self.color
    }

    fn default_model(&self) -> Option<&str> {
        Some(&self.model)
    }

    fn supports_image_editing(&self) -> bool {
        true
    }

    fn edit_image(
        &self,
        request: GenerationRequest,
    ) -> Pin<Box<dyn Future<Output = GenerationResult<GenerationOutput>> + Send + '_>> {
        Box::pin(super::edit::edit_image_impl(self, request))
    }
}
