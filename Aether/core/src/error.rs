/// Custom error types for Aether core library.
///
/// All errors in the Aether core are represented using this enum,
/// which provides clear error messages and integrates with UniFFI
/// for automatic conversion to Swift/Kotlin exceptions.
use thiserror::Error;

#[derive(Debug, Error)]
pub enum AetherError {
    /// Error occurred in hotkey listener subsystem
    #[error("Hotkey listener error: {0}")]
    HotkeyError(String),

    /// Error occurred during clipboard operations
    #[error("Clipboard error: {0}")]
    ClipboardError(String),

    /// Error occurred when invoking FFI callbacks
    #[error("FFI callback error: {0}")]
    CallbackError(String),

    /// Error occurred during configuration or database operations
    #[error("Configuration/Database error: {0}")]
    ConfigError(String),

    /// Generic error for other cases
    #[error("Aether error: {0}")]
    Other(String),
}

impl AetherError {
    /// Create a hotkey error with a message
    pub fn hotkey<S: Into<String>>(msg: S) -> Self {
        AetherError::HotkeyError(msg.into())
    }

    /// Create a clipboard error with a message
    pub fn clipboard<S: Into<String>>(msg: S) -> Self {
        AetherError::ClipboardError(msg.into())
    }

    /// Create a callback error with a message
    pub fn callback<S: Into<String>>(msg: S) -> Self {
        AetherError::CallbackError(msg.into())
    }

    /// Create a config/database error with a message
    pub fn config<S: Into<String>>(msg: S) -> Self {
        AetherError::ConfigError(msg.into())
    }

    /// Create a generic error with a message
    pub fn other<S: Into<String>>(msg: S) -> Self {
        AetherError::Other(msg.into())
    }
}

/// Type alias for Results using AetherError
pub type Result<T> = std::result::Result<T, AetherError>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_hotkey_error_creation() {
        let err = AetherError::hotkey("test error");
        assert!(matches!(err, AetherError::HotkeyError(_)));
        assert_eq!(err.to_string(), "Hotkey listener error: test error");
    }

    #[test]
    fn test_clipboard_error_creation() {
        let err = AetherError::clipboard("access denied");
        assert!(matches!(err, AetherError::ClipboardError(_)));
        assert_eq!(err.to_string(), "Clipboard error: access denied");
    }

    #[test]
    fn test_callback_error_creation() {
        let err = AetherError::callback("callback failed");
        assert!(matches!(err, AetherError::CallbackError(_)));
        assert_eq!(err.to_string(), "FFI callback error: callback failed");
    }

    #[test]
    fn test_error_display() {
        let err = AetherError::other("generic error");
        let display = format!("{}", err);
        assert_eq!(display, "Aether error: generic error");
    }

    #[test]
    fn test_error_debug() {
        let err = AetherError::hotkey("test");
        let debug = format!("{:?}", err);
        assert!(debug.contains("HotkeyError"));
    }
}
