//! Concrete [`VisionProvider`] implementations.
//!
//! - [`PlatformOcrProvider`] — delegates to the platform-native OCR engine via `desktop/*`

mod platform_ocr;

pub use platform_ocr::PlatformOcrProvider;
