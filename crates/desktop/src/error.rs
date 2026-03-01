//! Desktop capability error types.

use thiserror::Error;

/// Errors that can occur when using desktop capabilities.
#[derive(Debug, Error)]
pub enum DesktopError {
    /// The requested capability is not available on this platform or configuration.
    #[error("desktop capability not available: {0}")]
    NotAvailable(String),

    /// Screen capture failed.
    #[error("screen capture failed: {0}")]
    ScreenCapture(String),

    /// Input automation (mouse/keyboard) failed.
    #[error("input action failed: {0}")]
    InputFailed(String),

    /// OCR processing failed.
    #[error("OCR failed: {0}")]
    OcrFailed(String),

    /// Window management operation failed.
    #[error("window operation failed: {0}")]
    WindowFailed(String),

    /// The requested method is not yet implemented.
    #[error("not implemented: {0}")]
    NotImplemented(String),
}

/// Convenience result type for desktop operations.
pub type Result<T> = std::result::Result<T, DesktopError>;
