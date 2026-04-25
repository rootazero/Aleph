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

    /// The aleph-bridge Swift CLI binary failed or produced unexpected output.
    #[error("bridge failed: {0}")]
    BridgeFailed(String),

    /// The bridge has been disabled after too many crashes inside the
    /// restart window. Callers should surface this explicitly rather than
    /// retrying.
    #[error("desktop bridge disabled: {0}")]
    BridgeDisabled(String),

    /// A platform-level error occurred (e.g. IOPMAssertion failure).
    #[error("platform error: {0}")]
    PlatformError(String),

    /// A TCC permission was denied. The embedded `PermissionGuide` carries a
    /// deep link, human-readable steps, and a rationale that the LLM should
    /// relay to the user instead of just saying "permission denied".
    #[error("permission denied: {kind:?}")]
    PermissionDenied {
        kind: aleph_protocol::desktop_bridge::methods::perm::PermissionKind,
        guide: Box<aleph_protocol::desktop_bridge::methods::perm::PermissionGuide>,
    },
}

/// Convenience result type for desktop operations.
pub type Result<T> = std::result::Result<T, DesktopError>;
