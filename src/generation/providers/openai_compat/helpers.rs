//! Helper functions for OpenAI-compatible provider
//!
//! Contains utility functions for error parsing and request building.

use crate::generation::{GenerationError, GenerationParams, GenerationRequest};

use super::provider::OpenAiCompatProvider;
use super::types::{
    ImageGenerationRequest, ImageUrlRef, OpenAiErrorResponse, VideoContentPart, VideoTaskRequest,
};

impl OpenAiCompatProvider {
    /// Get the full URL for the generations endpoint
    /// Endpoint is auto-completed from base URL during build (see `builder::normalize_endpoint`)
    pub(crate) fn generations_url(&self) -> String {
        self.endpoint.clone()
    }

    /// True when this provider speaks the Volcengine Ark protocol.
    ///
    /// Ark differs from the OpenAI convention in two ways the base code must
    /// account for: image-to-image reuses the unified `/images/generations`
    /// JSON endpoint with an `image` field (instead of the multipart
    /// `/images/edits` endpoint), and video is a task-based async API. Both
    /// are reachable under the `*.volces.com` host.
    pub(crate) fn is_ark_protocol(&self) -> bool {
        self.endpoint.contains("volces.com")
    }

    /// True when this provider targets SiliconFlow.
    ///
    /// SiliconFlow's image API diverges from the OpenAI convention in three
    /// ways the base code must account for: dimensions go in `image_size`
    /// (not `size`), there is no `response_format` field, and the response
    /// wraps results under `images` (not `data`, handled by a serde alias).
    /// Detected by host so it works whether the preset uses the bare domain
    /// or a full endpoint URL.
    pub(crate) fn is_siliconflow(&self) -> bool {
        self.endpoint.contains("siliconflow")
    }

    /// Get the full URL for the edits endpoint
    /// Uses explicit `edit_endpoint` if set, otherwise derives from generations URL
    pub(crate) fn edits_url(&self) -> String {
        if let Some(ref edit_url) = self.edit_endpoint {
            return edit_url.clone();
        }
        // Heuristic for standard OpenAI-style URLs: /generations → /edits
        // For non-standard URLs, returns unchanged (those providers don't support editing)
        self.endpoint.replace("/generations", "/edits")
    }

    /// Build the API request body from a `GenerationRequest`
    pub(crate) fn build_request_body(&self, request: &GenerationRequest) -> ImageGenerationRequest {
        let model = request
            .params
            .model
            .clone()
            .unwrap_or_else(|| self.model.clone());

        // Build size string from width/height if provided
        let size = match (request.params.width, request.params.height) {
            (Some(w), Some(h)) => Some(format!("{w}x{h}")),
            _ => None,
        };

        // SiliconFlow names the dimension field `image_size` and has no
        // `response_format`; OpenAI/Ark use `size` + `response_format`.
        let is_siliconflow = self.is_siliconflow();

        // Ark's unified endpoint accepts a reference image for image-to-image
        // on the same generations call. Only forward it for Ark to avoid
        // sending an unsupported field to OpenAI-style providers.
        let image = if self.is_ark_protocol() {
            request
                .params
                .reference_image
                .as_ref()
                .filter(|s| !s.is_empty())
                .map(|s| normalize_image_ref(s))
        } else {
            None
        };

        ImageGenerationRequest {
            model,
            prompt: request.prompt.clone(),
            image,
            size: if is_siliconflow { None } else { size.clone() },
            image_size: if is_siliconflow { size } else { None },
            quality: request.params.quality.clone(),
            style: request.params.style.clone(),
            n: request.params.n,
            response_format: if is_siliconflow {
                None
            } else {
                Some("url".to_string()) // Default to URL format
            },
            user: request.user_id.clone(),
        }
    }

    /// Build the async video task request body from a `GenerationRequest`.
    ///
    /// Emits both conventions so one body works with either backend behind a
    /// generic proxy: the Volcengine Ark `content` array (text prompt with
    /// Seedance `--flag value` suffixes plus an optional reference image) and a
    /// top-level `prompt` string for OpenAI-style video proxies (e.g. T8star)
    /// that require it. The `prompt` mirrors the first content text part.
    pub(crate) fn build_video_task_body(&self, request: &GenerationRequest) -> VideoTaskRequest {
        let model = request
            .params
            .model
            .clone()
            .unwrap_or_else(|| self.model.clone());

        let prompt_text = build_video_prompt_text(&request.prompt, &request.params);

        let mut content = vec![VideoContentPart::Text {
            text: prompt_text.clone(),
        }];

        if let Some(reference) = request
            .params
            .reference_image
            .as_ref()
            .filter(|s| !s.is_empty())
        {
            content.push(VideoContentPart::ImageUrl {
                image_url: ImageUrlRef {
                    url: normalize_image_ref(reference),
                },
            });
        }

        VideoTaskRequest {
            model,
            prompt: prompt_text,
            content,
        }
    }

    /// Parse API error response and convert to `GenerationError`
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
                format!("Server error: {body}"),
                Some(status.as_u16()),
                &self.name,
            ),
            _ => GenerationError::provider(
                format!("Unexpected error: {body}"),
                Some(status.as_u16()),
                &self.name,
            ),
        }
    }
}

/// Normalize a reference image into a value the Ark API accepts.
///
/// HTTP(S) URLs and `data:` URLs pass through unchanged; a bare base64 string
/// is wrapped as a JPEG `data:` URL (Ark rejects unprefixed base64).
pub(crate) fn normalize_image_ref(reference: &str) -> String {
    let lower = reference.trim_start().to_lowercase();
    if lower.starts_with("http://") || lower.starts_with("https://") || lower.starts_with("data:") {
        reference.to_string()
    } else {
        format!("data:image/jpeg;base64,{reference}")
    }
}

/// Map a request's pixel dimensions to a Seedance resolution token.
///
/// Seedance accepts coarse resolution tiers rather than exact pixel sizes;
/// height (falling back to width) is bucketed to the nearest supported tier.
fn resolution_token(params: &GenerationParams) -> Option<&'static str> {
    let dim = params.height.or(params.width)?;
    Some(if dim <= 480 {
        "480p"
    } else if dim <= 720 {
        "720p"
    } else {
        "1080p"
    })
}

/// Build a Seedance video prompt, appending `--flag value` parameter suffixes.
///
/// Seedance encodes generation parameters (aspect ratio, resolution, duration,
/// seed) as text flags on the prompt rather than as JSON fields. Only
/// well-supported flags are emitted; unset params fall back to API defaults.
fn build_video_prompt_text(prompt: &str, params: &GenerationParams) -> String {
    let mut text = prompt.trim().to_string();

    if let Some(ratio) = params.aspect_ratio.as_deref().filter(|s| !s.is_empty()) {
        text.push_str(&format!(" --ratio {ratio}"));
    }
    if let Some(resolution) = resolution_token(params) {
        text.push_str(&format!(" --resolution {resolution}"));
    }
    if let Some(duration) = params.duration_seconds.filter(|d| *d > 0.0) {
        text.push_str(&format!(" --duration {}", duration.round() as i64));
    }
    if let Some(seed) = params.seed.filter(|s| *s >= 0) {
        text.push_str(&format!(" --seed {seed}"));
    }

    text
}
