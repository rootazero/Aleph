use thiserror::Error;

/// Errors that can occur during vision operations.
#[derive(Debug, Clone, Error)]
#[non_exhaustive]
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

    /// No provider supports the requested capability.
    #[error("No provider supports {0}")]
    UnsupportedCapability(String),
}
