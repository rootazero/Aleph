//! Vision operations for AetherCore
//!
//! Provides screen understanding capabilities using AI vision models.

use crate::config::Config;
use crate::error::Result;
use crate::vision::{VisionRequest, VisionResult, VisionService};
use tracing::{debug, error, info};

use super::AetherCore;

impl AetherCore {
    /// Process a vision request (OCR, description, or context-aware)
    ///
    /// Uses the user-configured default AI provider for vision processing.
    /// The provider must support vision capabilities (e.g., Claude, GPT-4o, Gemini).
    ///
    /// # Arguments
    ///
    /// * `request` - Vision request containing image data and task type
    ///
    /// # Returns
    ///
    /// * `Ok(VisionResult)` - Processing result with extracted text/description
    /// * `Err(AetherError)` - Processing error (provider not configured, not vision-capable, etc.)
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// let request = VisionRequest {
    ///     image_data: png_bytes,
    ///     capture_mode: CaptureMode::Region,
    ///     task: VisionTask::OcrOnly,
    ///     prompt: None,
    /// };
    /// let result = core.process_vision(request).await?;
    /// println!("Extracted: {}", result.extracted_text);
    /// ```
    pub async fn process_vision(&self, request: VisionRequest) -> Result<VisionResult> {
        // Clone config to avoid holding lock across await
        let config: Config = {
            let guard = self.config.lock().unwrap_or_else(|e| e.into_inner());
            guard.clone()
        };

        // Create VisionService with default config
        let vision_service = VisionService::with_defaults();

        // Process the request
        vision_service.process_vision(request, &config).await
    }

    /// Convenience method: Extract text from image (OCR only)
    ///
    /// This is a simplified wrapper around `process_vision` for pure text extraction.
    /// Uses the user-configured default AI provider.
    ///
    /// # Arguments
    ///
    /// * `image_data` - Raw PNG image data
    ///
    /// # Returns
    ///
    /// * `Ok(String)` - Extracted text
    /// * `Err(AetherError)` - Processing error
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// let text = core.extract_text(png_bytes).await?;
    /// println!("OCR result: {}", text);
    /// ```
    pub async fn extract_text(&self, image_data: Vec<u8>) -> Result<String> {
        info!(
            image_size = image_data.len(),
            "[OCR] extract_text START"
        );

        // Clone config to avoid holding lock across await
        let config: Config = {
            let guard = self.config.lock().unwrap_or_else(|e| e.into_inner());
            guard.clone()
        };

        // Log config details for debugging
        let default_provider = config.general.default_provider.as_ref();
        info!(
            default_provider = ?default_provider,
            provider_count = config.providers.len(),
            "[OCR] Config loaded"
        );

        if default_provider.is_none() {
            error!("[OCR] No default_provider configured in [general] section");
            return Err(crate::error::AetherError::invalid_config(
                "No default_provider configured. Set default_provider in [general] section of config.toml"
            ));
        }

        let provider_name = default_provider.unwrap();
        if let Some(provider_config) = config.providers.get(provider_name) {
            info!(
                provider = %provider_name,
                provider_type = ?provider_config.provider_type,
                model = %provider_config.model,
                has_api_key = provider_config.api_key.is_some(),
                "[OCR] Provider config found"
            );
        } else {
            error!(
                provider = %provider_name,
                "[OCR] Provider '{}' not found in [providers] section",
                provider_name
            );
            return Err(crate::error::AetherError::invalid_config(format!(
                "Provider '{}' not found in [providers] section of config.toml",
                provider_name
            )));
        }

        // Create VisionService with default config
        let vision_service = VisionService::with_defaults();

        // Use the core's runtime to execute the async code
        // This ensures Tokio reactor is available for HTTP calls (reqwest)
        info!("[OCR] Spawning vision task on tokio runtime...");
        let runtime = self.runtime.clone();

        match runtime
            .spawn(async move { vision_service.extract_text(image_data, &config).await })
            .await
        {
            Ok(Ok(text)) => {
                info!(
                    result_length = text.len(),
                    "[OCR] extract_text SUCCESS"
                );
                debug!(
                    result_preview = %text.chars().take(100).collect::<String>(),
                    "[OCR] Result preview"
                );
                Ok(text)
            }
            Ok(Err(e)) => {
                error!(
                    error = %e,
                    "[OCR] VisionService returned error"
                );
                Err(e)
            }
            Err(e) => {
                error!(
                    error = %e,
                    "[OCR] Task join error"
                );
                Err(crate::error::AetherError::other(format!("Task join error: {}", e)))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vision::{CaptureMode, VisionTask};

    // Note: Full integration tests require a mock provider setup
    // These tests verify the API structure only

    #[test]
    fn test_vision_request_creation() {
        let request = VisionRequest {
            image_data: vec![0, 1, 2, 3],
            capture_mode: CaptureMode::Region,
            task: VisionTask::OcrOnly,
            prompt: None,
        };
        assert_eq!(request.image_data.len(), 4);
        assert!(request.prompt.is_none());
    }

    #[test]
    fn test_vision_result_default() {
        let result = VisionResult::default();
        assert!(result.extracted_text.is_empty());
        assert!(result.description.is_none());
        assert!(result.ai_response.is_none());
    }
}
