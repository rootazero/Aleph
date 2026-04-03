//! Concrete [`VisionProvider`] implementations.
//!
//! - [`ClaudeVisionProvider`] — delegates to Claude's multimodal API
//! - [`PlatformOcrProvider`] — delegates to the platform-native OCR engine via `desktop/*`

mod claude;
mod platform_ocr;

pub use claude::ClaudeVisionProvider;
pub use platform_ocr::PlatformOcrProvider;
