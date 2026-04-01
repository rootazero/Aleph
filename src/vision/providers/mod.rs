//! Concrete [`VisionProvider`] implementations.
//!
//! - [`ClaudeVisionProvider`] — delegates to Claude's multimodal API
//! - [`PlatformOcrProvider`] — delegates to macOS Vision framework via Desktop Bridge

mod claude;
mod platform_ocr;

pub use claude::ClaudeVisionProvider;
pub use platform_ocr::PlatformOcrProvider;
