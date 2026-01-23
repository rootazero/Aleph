//! Image editing implementation for OpenAI-compatible provider
//!
//! Contains the `GenerationProvider::edit_image` implementation.

use base64::Engine;
use std::time::{Duration, Instant};
use tracing::{debug, error, info};

use crate::generation::{
    GenerationData, GenerationError, GenerationMetadata, GenerationOutput, GenerationRequest,
    GenerationResult, GenerationType,
};

use super::provider::OpenAiCompatProvider;
use super::types::{ImageGenerationResponse, DEFAULT_TIMEOUT_SECS};

/// Implementation of image editing for OpenAI-compatible API
pub(crate) async fn edit_image_impl(
    provider: &OpenAiCompatProvider,
    request: GenerationRequest,
) -> GenerationResult<GenerationOutput> {
    // Validate generation type
    if request.generation_type != GenerationType::Image {
        return Err(GenerationError::unsupported_generation_type(
            request.generation_type.to_string(),
            &provider.name,
        ));
    }

    // Require reference image
    let reference_image = request.params.reference_image.as_ref().ok_or_else(|| {
        GenerationError::invalid_parameters(
            "reference_image is required for image editing",
            Some("reference_image".to_string()),
        )
    })?;

    let start_time = Instant::now();
    let request_id = request.request_id.clone();

    debug!(
        provider = %provider.name,
        prompt = %request.prompt,
        model = %provider.model,
        "Starting OpenAI-compatible image editing"
    );

    // Build multipart form
    let mut form = reqwest::multipart::Form::new();

    // Add model
    let model = request
        .params
        .model
        .clone()
        .unwrap_or_else(|| provider.model.clone());
    form = form.text("model", model.clone());

    // Add prompt
    form = form.text("prompt", request.prompt.clone());

    // Add image - handle both base64 and URL
    if reference_image.starts_with("http://") || reference_image.starts_with("https://") {
        // Download image from URL first
        let image_bytes = provider
            .client
            .get(reference_image)
            .send()
            .await
            .map_err(|e| GenerationError::network(format!("Failed to download image: {}", e)))?
            .bytes()
            .await
            .map_err(|e| GenerationError::network(format!("Failed to read image bytes: {}", e)))?;

        let part = reqwest::multipart::Part::bytes(image_bytes.to_vec())
            .file_name("image.png")
            .mime_str("image/png")
            .map_err(|e| GenerationError::invalid_parameters(e.to_string(), None))?;
        form = form.part("image", part);
    } else {
        // Assume base64-encoded data
        let image_bytes = base64::engine::general_purpose::STANDARD
            .decode(reference_image)
            .map_err(|e| {
                GenerationError::invalid_parameters(
                    format!("Invalid base64 image data: {}", e),
                    Some("reference_image".to_string()),
                )
            })?;

        let part = reqwest::multipart::Part::bytes(image_bytes)
            .file_name("image.png")
            .mime_str("image/png")
            .map_err(|e| GenerationError::invalid_parameters(e.to_string(), None))?;
        form = form.part("image", part);
    }

    // Add optional mask if provided via extra params
    if let Some(mask_value) = request.params.extra.get("mask") {
        if let Some(mask_str) = mask_value.as_str() {
            let mask_bytes = base64::engine::general_purpose::STANDARD
                .decode(mask_str)
                .map_err(|e| {
                    GenerationError::invalid_parameters(
                        format!("Invalid base64 mask data: {}", e),
                        Some("mask".to_string()),
                    )
                })?;

            let part = reqwest::multipart::Part::bytes(mask_bytes)
                .file_name("mask.png")
                .mime_str("image/png")
                .map_err(|e| GenerationError::invalid_parameters(e.to_string(), None))?;
            form = form.part("mask", part);
        }
    }

    // Add optional size
    if let (Some(w), Some(h)) = (request.params.width, request.params.height) {
        form = form.text("size", format!("{}x{}", w, h));
    }

    // Add optional n (number of images)
    if let Some(n) = request.params.n {
        form = form.text("n", n.to_string());
    }

    // Add response format
    form = form.text("response_format", "url");

    // Add optional user
    if let Some(user) = &request.user_id {
        form = form.text("user", user.clone());
    }

    let url = provider.edits_url();
    debug!(url = %url, "Sending edit request to OpenAI-compatible API");

    // Make API request
    let response = provider
        .client
        .post(&url)
        .header("Authorization", format!("Bearer {}", provider.api_key))
        .multipart(form)
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
            provider = %provider.name,
            status = %status,
            body = %response_text,
            "OpenAI-compatible image edit request failed"
        );
        return Err(provider.parse_error_response(status, &response_text));
    }

    // Parse successful response (same format as generations)
    let api_response: ImageGenerationResponse =
        serde_json::from_str(&response_text).map_err(|e| {
            error!(
                error = %e,
                body = %response_text,
                "Failed to parse OpenAI-compatible edit response"
            );
            GenerationError::serialization(format!("Failed to parse response: {}", e))
        })?;

    // Extract first image
    let first_image = api_response.data.first().ok_or_else(|| {
        GenerationError::provider("No images in response", None, &provider.name)
    })?;

    // Convert to GenerationData
    let data = if let Some(url) = &first_image.url {
        GenerationData::url(url.clone())
    } else if let Some(b64) = &first_image.b64_json {
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
            &provider.name,
        ));
    };

    // Build metadata
    let duration = start_time.elapsed();
    let mut metadata = GenerationMetadata::new()
        .with_provider(&provider.name)
        .with_model(model)
        .with_duration(duration);

    if let Some(revised) = &first_image.revised_prompt {
        metadata = metadata.with_revised_prompt(revised.clone());
    }

    if let (Some(w), Some(h)) = (request.params.width, request.params.height) {
        metadata = metadata.with_dimensions(w, h);
    }

    info!(
        provider = %provider.name,
        duration_ms = duration.as_millis(),
        "OpenAI-compatible image editing completed"
    );

    let mut output =
        GenerationOutput::new(request.generation_type, data).with_metadata(metadata);

    if let Some(id) = request_id {
        output = output.with_request_id(id);
    }

    // Handle additional images
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
}
