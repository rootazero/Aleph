//! Vision configuration
//!
//! Defines configuration options for the vision capability.

use super::prompt::{DEFAULT_DESCRIBE_PROMPT, DEFAULT_OCR_PROMPT, DEFAULT_OCR_WITH_CONTEXT_PROMPT};

/// Vision service configuration
#[derive(Debug, Clone)]
pub struct VisionConfig {
    /// Maximum image dimension (width or height) in pixels
    /// Images larger than this will be resized while preserving aspect ratio
    pub max_image_dimension: u32,

    /// JPEG compression quality (1-100)
    /// Higher values = better quality but larger file size
    pub jpeg_quality: u8,

    /// OCR prompt template for text extraction
    pub ocr_prompt: String,

    /// Description prompt template for image description
    pub describe_prompt: String,

    /// OCR with context prompt template
    pub ocr_with_context_prompt: String,
}

impl Default for VisionConfig {
    fn default() -> Self {
        Self {
            max_image_dimension: 2048,
            jpeg_quality: 85,
            ocr_prompt: DEFAULT_OCR_PROMPT.to_string(),
            describe_prompt: DEFAULT_DESCRIBE_PROMPT.to_string(),
            ocr_with_context_prompt: DEFAULT_OCR_WITH_CONTEXT_PROMPT.to_string(),
        }
    }
}

impl VisionConfig {
    /// Create a new VisionConfig with custom settings
    pub fn new(max_image_dimension: u32, jpeg_quality: u8) -> Self {
        Self {
            max_image_dimension,
            jpeg_quality,
            ..Default::default()
        }
    }

    /// Set custom OCR prompt
    pub fn with_ocr_prompt(mut self, prompt: impl Into<String>) -> Self {
        self.ocr_prompt = prompt.into();
        self
    }

    /// Set custom description prompt
    pub fn with_describe_prompt(mut self, prompt: impl Into<String>) -> Self {
        self.describe_prompt = prompt.into();
        self
    }

    /// Validate configuration
    pub fn validate(&self) -> Result<(), String> {
        if self.max_image_dimension < 100 {
            return Err("max_image_dimension must be at least 100".to_string());
        }
        if self.max_image_dimension > 8192 {
            return Err("max_image_dimension must be at most 8192".to_string());
        }
        if self.jpeg_quality < 1 || self.jpeg_quality > 100 {
            return Err("jpeg_quality must be between 1 and 100".to_string());
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_config() {
        let config = VisionConfig::default();
        assert_eq!(config.max_image_dimension, 2048);
        assert_eq!(config.jpeg_quality, 85);
        assert!(!config.ocr_prompt.is_empty());
    }

    #[test]
    fn test_config_validation() {
        let config = VisionConfig::default();
        assert!(config.validate().is_ok());

        let invalid_config = VisionConfig {
            max_image_dimension: 50, // Too small
            ..Default::default()
        };
        assert!(invalid_config.validate().is_err());

        let invalid_quality = VisionConfig {
            jpeg_quality: 0, // Invalid
            ..Default::default()
        };
        assert!(invalid_quality.validate().is_err());
    }

    #[test]
    fn test_config_builder() {
        let config = VisionConfig::new(1920, 90)
            .with_ocr_prompt("Custom OCR prompt");

        assert_eq!(config.max_image_dimension, 1920);
        assert_eq!(config.jpeg_quality, 90);
        assert_eq!(config.ocr_prompt, "Custom OCR prompt");
    }
}
