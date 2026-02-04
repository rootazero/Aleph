//! Vision capability for Aether
//!
//! This module provides native screen understanding capabilities using AI vision models.
//! It supports OCR text extraction and image description using the user's configured
//! default AI provider.
//!
//! # Architecture
//!
//! ```text
//! Swift (Screen Capture) → Rust (Image Processing) → AI Provider → Result
//! ```
//!
//! # Features
//!
//! - **OCR Text Extraction**: Extract text from screenshots using AI vision
//! - **Image Description**: Generate descriptions of captured images
//! - **Context-Aware OCR**: Extract text and use as AI conversation context
//! - **Automatic Image Optimization**: Resize and compress images for API efficiency
//!
//! # Example
//!
//! ```rust,ignore
//! use alephcore::vision::{VisionService, VisionRequest, VisionTask, CaptureMode};
//!
//! let service = VisionService::new(config, provider_registry);
//! let request = VisionRequest {
//!     image_data: png_bytes,
//!     capture_mode: CaptureMode::Region,
//!     task: VisionTask::OcrOnly,
//!     prompt: None,
//! };
//! let result = service.process_vision(request).await?;
//! println!("Extracted text: {}", result.extracted_text);
//! ```

mod config;
mod prompt;
mod service;

pub use config::VisionConfig;
pub use service::VisionService;

// FFI types for UniFFI
// These are defined here to be re-exported in lib.rs

/// Capture mode for screen capture
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CaptureMode {
    /// User-selected region
    Region,
    /// Active window capture
    Window,
    /// Full screen capture
    FullScreen,
}

/// Vision task type
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VisionTask {
    /// Extract text only (OCR)
    OcrOnly,
    /// Extract text and use as AI context
    OcrWithContext,
    /// Generate image description
    Describe,
}

/// Vision request from Swift to Rust
#[derive(Debug, Clone)]
pub struct VisionRequest {
    /// Raw PNG image data
    pub image_data: Vec<u8>,
    /// Capture mode used
    pub capture_mode: CaptureMode,
    /// Task to perform
    pub task: VisionTask,
    /// Optional user prompt for OcrWithContext mode
    pub prompt: Option<String>,
}

/// Vision result from Rust to Swift
#[derive(Debug, Clone)]
pub struct VisionResult {
    /// OCR extracted text
    pub extracted_text: String,
    /// Image description (if requested)
    pub description: Option<String>,
    /// AI response (OcrWithContext mode)
    pub ai_response: Option<String>,
    /// Confidence score (0.0-1.0)
    pub confidence: f32,
    /// Processing duration in milliseconds
    pub processing_time_ms: u64,
}

impl Default for VisionResult {
    fn default() -> Self {
        Self {
            extracted_text: String::new(),
            description: None,
            ai_response: None,
            confidence: 0.0,
            processing_time_ms: 0,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_capture_mode() {
        assert_eq!(CaptureMode::Region, CaptureMode::Region);
        assert_ne!(CaptureMode::Region, CaptureMode::Window);
    }

    #[test]
    fn test_vision_task() {
        assert_eq!(VisionTask::OcrOnly, VisionTask::OcrOnly);
        assert_ne!(VisionTask::OcrOnly, VisionTask::Describe);
    }

    #[test]
    fn test_vision_result_default() {
        let result = VisionResult::default();
        assert!(result.extracted_text.is_empty());
        assert!(result.description.is_none());
        assert!(result.ai_response.is_none());
        assert_eq!(result.confidence, 0.0);
        assert_eq!(result.processing_time_ms, 0);
    }
}
