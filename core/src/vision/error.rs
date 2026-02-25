use thiserror::Error;

/// Errors that can occur during vision operations.
#[derive(Debug, Error)]
pub enum VisionError {
    /// No vision provider has been configured or registered.
    #[error("No vision provider configured")]
    NoProvider,

    /// A vision provider returned an error during processing.
    #[error("Vision provider error: {0}")]
    ProviderError(String),

    /// Failed to decode or process image data.
    #[error("Image decode error: {0}")]
    ImageError(String),

    /// OCR functionality is not available on this platform.
    #[error("OCR not available on this platform")]
    OcrNotAvailable,

    /// The provided image format is not supported.
    #[error("Unsupported image format: {0}")]
    UnsupportedFormat(String),
}
