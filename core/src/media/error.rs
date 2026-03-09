//! Media processing error types.

use thiserror::Error;

/// Errors that can occur during media processing.
#[derive(Debug, Error)]
pub enum MediaError {
    /// No provider configured for this media type.
    #[error("No media provider available for {media_type}")]
    NoProvider { media_type: String },

    /// A provider returned an error.
    #[error("Media provider error [{provider}]: {message}")]
    ProviderError { provider: String, message: String },

    /// File exceeds size policy.
    #[error("Media exceeds size limit: {message}")]
    SizeLimitExceeded { message: String },

    /// Unsupported format.
    #[error("Unsupported media format: {0}")]
    UnsupportedFormat(String),

    /// Format detection failed.
    #[error("Cannot detect media format: {0}")]
    DetectionFailed(String),

    /// I/O error reading file.
    #[error("I/O error: {0}")]
    IoError(String),
}

impl From<std::io::Error> for MediaError {
    fn from(err: std::io::Error) -> Self {
        MediaError::IoError(err.to_string())
    }
}
