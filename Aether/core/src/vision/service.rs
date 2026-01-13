//! Vision service implementation
//!
//! Provides the core vision processing functionality using AI providers.

use super::config::VisionConfig;
use super::prompt::build_prompt;
use super::{CaptureMode, VisionRequest, VisionResult, VisionTask};
use crate::clipboard::{ImageData, ImageFormat as ClipboardImageFormat};
use crate::config::Config;
use crate::error::{AetherError, Result};
use crate::providers::{create_provider, AiProvider};
use image::{DynamicImage, GenericImageView, ImageFormat};
use std::io::Cursor;
use std::sync::Arc;
use std::time::Instant;
use tracing::{debug, info};

/// Processed image ready for AI provider
struct ProcessedImage {
    /// Raw image bytes (JPEG encoded)
    data: Vec<u8>,
    /// Original dimensions
    original_width: u32,
    original_height: u32,
    /// Processed dimensions
    processed_width: u32,
    processed_height: u32,
}

/// Vision service for screen understanding
///
/// Uses the user-configured default AI provider to process images
/// for OCR text extraction and image description.
pub struct VisionService {
    /// Vision configuration
    config: VisionConfig,
}

impl VisionService {
    /// Create a new VisionService with the given configuration
    pub fn new(config: VisionConfig) -> Self {
        Self { config }
    }

    /// Create a VisionService with default configuration
    pub fn with_defaults() -> Self {
        Self::new(VisionConfig::default())
    }

    /// Process a vision request
    ///
    /// # Arguments
    ///
    /// * `request` - Vision request containing image data and task
    /// * `app_config` - Application configuration to get default provider
    ///
    /// # Returns
    ///
    /// * `Ok(VisionResult)` - Processing result with extracted text/description
    /// * `Err(AetherError)` - Processing error
    pub async fn process_vision(
        &self,
        request: VisionRequest,
        app_config: &Config,
    ) -> Result<VisionResult> {
        let start = Instant::now();

        info!(
            task = ?request.task,
            capture_mode = ?request.capture_mode,
            image_size = request.image_data.len(),
            "Processing vision request"
        );

        // 1. Get the user-configured default provider
        let provider = self.get_vision_provider(app_config)?;

        // 2. Check if provider supports vision
        if !provider.supports_vision() {
            return Err(AetherError::provider(format!(
                "Provider '{}' does not support vision/image input. Please configure a vision-capable provider (e.g., Claude, GPT-4o, Gemini) as your default.",
                provider.name()
            )));
        }

        // 3. Preprocess image
        let processed = self.preprocess_image(&request.image_data)?;

        debug!(
            original = format!("{}x{}", processed.original_width, processed.original_height),
            processed = format!("{}x{}", processed.processed_width, processed.processed_height),
            data_size = processed.data.len(),
            "Image preprocessed"
        );

        // 4. Build prompt based on task
        let prompt = build_prompt(&request.task, &self.config, request.prompt.as_deref());

        // 5. Create ImageData for provider
        let image_data = ImageData::new(processed.data, ClipboardImageFormat::Jpeg);

        // 6. Call AI provider
        let response = provider
            .process_with_image(&prompt, Some(&image_data), None)
            .await?;

        // 7. Parse response based on task
        let result = self.parse_response(&request.task, response, start.elapsed().as_millis() as u64);

        info!(
            task = ?request.task,
            text_length = result.extracted_text.len(),
            processing_time_ms = result.processing_time_ms,
            "Vision request completed"
        );

        Ok(result)
    }

    /// Convenience method: Extract text from image (OCR only)
    ///
    /// # Arguments
    ///
    /// * `image_data` - Raw PNG image data
    /// * `app_config` - Application configuration to get default provider
    ///
    /// # Returns
    ///
    /// * `Ok(String)` - Extracted text
    /// * `Err(AetherError)` - Processing error
    pub async fn extract_text(&self, image_data: Vec<u8>, app_config: &Config) -> Result<String> {
        let request = VisionRequest {
            image_data,
            capture_mode: CaptureMode::Region,
            task: VisionTask::OcrOnly,
            prompt: None,
        };

        let result = self.process_vision(request, app_config).await?;
        Ok(result.extracted_text)
    }

    /// Get the vision-capable AI provider from user configuration
    fn get_vision_provider(&self, config: &Config) -> Result<Arc<dyn AiProvider>> {
        debug!("[OCR-Vision] get_vision_provider START");

        // Get default provider name from config
        let default_provider_name = config
            .general
            .default_provider
            .as_ref()
            .ok_or_else(|| {
                tracing::error!("[OCR-Vision] No default_provider in config.general");
                AetherError::invalid_config(
                    "No default provider configured. Please set a default provider in settings."
                        .to_string(),
                )
            })?;

        debug!(
            provider = %default_provider_name,
            "[OCR-Vision] Default provider name"
        );

        // Get provider config
        let provider_config = config
            .providers
            .get(default_provider_name)
            .ok_or_else(|| {
                tracing::error!(
                    provider = %default_provider_name,
                    available_providers = ?config.providers.keys().collect::<Vec<_>>(),
                    "[OCR-Vision] Provider not found in config.providers"
                );
                AetherError::invalid_config(format!(
                    "Default provider '{}' not found in configuration. Available: {:?}",
                    default_provider_name,
                    config.providers.keys().collect::<Vec<_>>()
                ))
            })?;

        debug!(
            provider = %default_provider_name,
            enabled = provider_config.enabled,
            model = %provider_config.model,
            provider_type = ?provider_config.provider_type,
            "[OCR-Vision] Provider config loaded"
        );

        // Check if provider is enabled
        if !provider_config.enabled {
            tracing::error!(
                provider = %default_provider_name,
                "[OCR-Vision] Provider is disabled"
            );
            return Err(AetherError::invalid_config(format!(
                "Default provider '{}' is disabled",
                default_provider_name
            )));
        }

        // Create provider instance
        debug!("[OCR-Vision] Creating provider instance...");
        let provider = create_provider(default_provider_name, provider_config.clone())?;

        // Check vision capability
        let supports_vision = provider.supports_vision();
        info!(
            provider = %default_provider_name,
            supports_vision = supports_vision,
            "[OCR-Vision] Provider created"
        );

        if !supports_vision {
            tracing::error!(
                provider = %default_provider_name,
                "[OCR-Vision] Provider does not support vision!"
            );
        }

        Ok(provider)
    }

    /// Preprocess image: resize if needed and convert to JPEG
    fn preprocess_image(&self, data: &[u8]) -> Result<ProcessedImage> {
        // Load image from memory
        let img = image::load_from_memory(data).map_err(|e| {
            AetherError::other(format!("Failed to load image: {}", e))
        })?;

        let original_width = img.width();
        let original_height = img.height();

        // Resize if needed (preserve aspect ratio)
        let img = if original_width > self.config.max_image_dimension
            || original_height > self.config.max_image_dimension
        {
            self.resize_image(img)
        } else {
            img
        };

        let processed_width = img.width();
        let processed_height = img.height();

        // Convert to JPEG for smaller transfer size
        let mut buffer = Vec::new();
        let mut cursor = Cursor::new(&mut buffer);

        img.write_to(&mut cursor, ImageFormat::Jpeg)
            .map_err(|e| AetherError::other(format!("Failed to encode image: {}", e)))?;

        Ok(ProcessedImage {
            data: buffer,
            original_width,
            original_height,
            processed_width,
            processed_height,
        })
    }

    /// Resize image while preserving aspect ratio
    fn resize_image(&self, img: DynamicImage) -> DynamicImage {
        let max_dim = self.config.max_image_dimension;
        let (width, height) = img.dimensions();

        // Calculate new dimensions
        let (new_width, new_height) = if width > height {
            let ratio = max_dim as f64 / width as f64;
            (max_dim, (height as f64 * ratio) as u32)
        } else {
            let ratio = max_dim as f64 / height as f64;
            ((width as f64 * ratio) as u32, max_dim)
        };

        debug!(
            original = format!("{}x{}", width, height),
            new = format!("{}x{}", new_width, new_height),
            "Resizing image"
        );

        img.resize(new_width, new_height, image::imageops::FilterType::Lanczos3)
    }

    /// Parse AI response based on task type
    fn parse_response(&self, task: &VisionTask, response: String, processing_time_ms: u64) -> VisionResult {
        match task {
            VisionTask::OcrOnly => VisionResult {
                extracted_text: response.trim().to_string(),
                description: None,
                ai_response: None,
                confidence: 1.0, // AI models don't provide confidence scores
                processing_time_ms,
            },
            VisionTask::Describe => VisionResult {
                extracted_text: String::new(),
                description: Some(response.trim().to_string()),
                ai_response: None,
                confidence: 1.0,
                processing_time_ms,
            },
            VisionTask::OcrWithContext => VisionResult {
                extracted_text: String::new(), // Context mode doesn't separate OCR text
                description: None,
                ai_response: Some(response.trim().to_string()),
                confidence: 1.0,
                processing_time_ms,
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_vision_service_creation() {
        let service = VisionService::with_defaults();
        assert_eq!(service.config.max_image_dimension, 2048);
    }

    #[test]
    fn test_vision_service_custom_config() {
        let config = VisionConfig::new(1920, 90);
        let service = VisionService::new(config);
        assert_eq!(service.config.max_image_dimension, 1920);
        assert_eq!(service.config.jpeg_quality, 90);
    }

    #[test]
    fn test_parse_response_ocr_only() {
        let service = VisionService::with_defaults();
        let result = service.parse_response(
            &VisionTask::OcrOnly,
            "  Hello World  ".to_string(),
            100,
        );
        assert_eq!(result.extracted_text, "Hello World");
        assert!(result.description.is_none());
        assert!(result.ai_response.is_none());
        assert_eq!(result.processing_time_ms, 100);
    }

    #[test]
    fn test_parse_response_describe() {
        let service = VisionService::with_defaults();
        let result = service.parse_response(
            &VisionTask::Describe,
            "A screenshot of code".to_string(),
            200,
        );
        assert!(result.extracted_text.is_empty());
        assert_eq!(result.description, Some("A screenshot of code".to_string()));
        assert!(result.ai_response.is_none());
    }

    #[test]
    fn test_parse_response_ocr_with_context() {
        let service = VisionService::with_defaults();
        let result = service.parse_response(
            &VisionTask::OcrWithContext,
            "The error says: Connection refused".to_string(),
            300,
        );
        assert!(result.extracted_text.is_empty());
        assert!(result.description.is_none());
        assert_eq!(
            result.ai_response,
            Some("The error says: Connection refused".to_string())
        );
    }
}
