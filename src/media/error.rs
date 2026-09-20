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

    /// Path refused by the media trust-root guard.
    ///
    /// Defense-in-depth: the tool layer (`audio_transcribe`,
    /// `document_extract`) gates the model-supplied path with
    /// `check_and_resolve_path` from `file_ops`, and the providers
    /// (`AudioMediaProvider`, `TextDocumentProvider`) cross-check the path
    /// they actually open with `MediaCache::safe_local_media_path`. Both
    /// layers should agree; this variant is the loud failure when they
    /// don't (or when a caller bypasses the tool layer).
    #[error("Media path refused by trust-root guard: {0}")]
    Refused(String),
}
