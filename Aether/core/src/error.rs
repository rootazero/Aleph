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

    /// Network error during API calls
    #[error("Network error: {0}")]
    NetworkError(String),

    /// Authentication error (invalid API key)
    #[error("Authentication error: {0}")]
    AuthenticationError(String),

    /// Rate limit error (too many requests)
    #[error("Rate limit error: {0}")]
    RateLimitError(String),

    /// Provider-specific error (API returned error)
    #[error("Provider error: {0}")]
    ProviderError(String),

    /// Request timeout
    #[error("Request timed out")]
    Timeout,

    /// No provider available for routing
    #[error("No provider available")]
    NoProviderAvailable,

    /// Invalid configuration
    #[error("Invalid configuration: {0}")]
    InvalidConfig(String),

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

    /// Create a network error with a message
    pub fn network<S: Into<String>>(msg: S) -> Self {
        AetherError::NetworkError(msg.into())
    }

    /// Create an authentication error with a message
    pub fn authentication<S: Into<String>>(msg: S) -> Self {
        AetherError::AuthenticationError(msg.into())
    }

    /// Create a rate limit error with a message
    pub fn rate_limit<S: Into<String>>(msg: S) -> Self {
        AetherError::RateLimitError(msg.into())
    }

    /// Create a provider error with a message
    pub fn provider<S: Into<String>>(msg: S) -> Self {
        AetherError::ProviderError(msg.into())
    }

    /// Create an invalid config error with a message
    pub fn invalid_config<S: Into<String>>(msg: S) -> Self {
        AetherError::InvalidConfig(msg.into())
    }

    /// Create a generic error with a message
    pub fn other<S: Into<String>>(msg: S) -> Self {
        AetherError::Other(msg.into())
    }

    /// Get a user-friendly error message suitable for display in the UI
    ///
    /// This method converts technical error messages into friendly,
    /// actionable messages that users can understand and act upon.
    ///
    /// # Example
    ///
    /// ```
    /// use aethecore::error::AetherError;
    ///
    /// let err = AetherError::AuthenticationError("401 Unauthorized".into());
    /// assert_eq!(
    ///     err.user_friendly_message(),
    ///     "Authentication failed. Please check your API key in settings."
    /// );
    /// ```
    pub fn user_friendly_message(&self) -> String {
        match self {
            AetherError::AuthenticationError(_) => {
                "Authentication failed. Please check your API key in settings.".to_string()
            }
            AetherError::RateLimitError(_) => {
                "Rate limit exceeded. Please try again in a few moments.".to_string()
            }
            AetherError::NetworkError(_) => {
                "Network connection failed. Please check your internet connection.".to_string()
            }
            AetherError::Timeout => {
                "Request timed out. The AI service is taking too long to respond. Please try again."
                    .to_string()
            }
            AetherError::NoProviderAvailable => {
                "No AI provider is configured. Please configure at least one provider in settings."
                    .to_string()
            }
            AetherError::InvalidConfig(msg) => {
                format!("Configuration error: {}. Please check your settings.", msg)
            }
            AetherError::ProviderError(msg) => {
                // Check if it's a server error (5xx)
                if msg.contains("500")
                    || msg.contains("502")
                    || msg.contains("503")
                    || msg.contains("504")
                {
                    "The AI service is temporarily unavailable. Please try again later.".to_string()
                } else {
                    format!("AI service error: {}. Please try again.", msg)
                }
            }
            AetherError::HotkeyError(msg) => {
                format!(
                    "Hotkey error: {}. Please check your system permissions.",
                    msg
                )
            }
            AetherError::ClipboardError(msg) => {
                format!(
                    "Clipboard error: {}. Please check your system permissions.",
                    msg
                )
            }
            AetherError::ConfigError(msg) => {
                format!(
                    "Configuration error: {}. Please check your settings file.",
                    msg
                )
            }
            AetherError::CallbackError(msg) => {
                format!("Internal error: {}. Please restart the application.", msg)
            }
            AetherError::Other(msg) => {
                format!("An error occurred: {}. Please try again.", msg)
            }
        }
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

    #[test]
    fn test_network_error() {
        let err = AetherError::network("connection failed");
        assert!(matches!(err, AetherError::NetworkError(_)));
        assert_eq!(err.to_string(), "Network error: connection failed");
    }

    #[test]
    fn test_authentication_error() {
        let err = AetherError::authentication("invalid API key");
        assert!(matches!(err, AetherError::AuthenticationError(_)));
        assert_eq!(err.to_string(), "Authentication error: invalid API key");
    }

    #[test]
    fn test_rate_limit_error() {
        let err = AetherError::rate_limit("too many requests");
        assert!(matches!(err, AetherError::RateLimitError(_)));
        assert_eq!(err.to_string(), "Rate limit error: too many requests");
    }

    #[test]
    fn test_provider_error() {
        let err = AetherError::provider("API returned 500");
        assert!(matches!(err, AetherError::ProviderError(_)));
        assert_eq!(err.to_string(), "Provider error: API returned 500");
    }

    #[test]
    fn test_timeout_error() {
        let err = AetherError::Timeout;
        assert_eq!(err.to_string(), "Request timed out");
    }

    #[test]
    fn test_no_provider_available() {
        let err = AetherError::NoProviderAvailable;
        assert_eq!(err.to_string(), "No provider available");
    }

    #[test]
    fn test_invalid_config_error() {
        let err = AetherError::invalid_config("missing API key");
        assert!(matches!(err, AetherError::InvalidConfig(_)));
        assert_eq!(err.to_string(), "Invalid configuration: missing API key");
    }
}
