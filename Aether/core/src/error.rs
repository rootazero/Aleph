/// Custom error types for Aether core library.
///
/// All errors in the Aether core are represented using this enum,
/// which provides clear error messages and integrates with UniFFI
/// for automatic conversion to Swift/Kotlin exceptions.
use thiserror::Error;

#[derive(Debug, Error)]
pub enum AetherError {
    /// Error occurred in hotkey listener subsystem
    #[error("Hotkey listener error: {message}")]
    HotkeyError {
        message: String,
        suggestion: Option<String>,
    },

    /// Error occurred during clipboard operations
    #[error("Clipboard error: {message}")]
    ClipboardError {
        message: String,
        suggestion: Option<String>,
    },

    /// Error occurred during input simulation (keyboard/mouse)
    #[error("Input simulation error: {message}")]
    InputSimulationError {
        message: String,
        suggestion: Option<String>,
    },

    /// Error occurred when invoking FFI callbacks
    #[error("FFI callback error: {message}")]
    CallbackError {
        message: String,
        suggestion: Option<String>,
    },

    /// Error occurred during configuration or database operations
    #[error("Configuration/Database error: {message}")]
    ConfigError {
        message: String,
        suggestion: Option<String>,
    },

    /// Network error during API calls
    #[error("Network error: {message}")]
    NetworkError {
        message: String,
        suggestion: Option<String>,
    },

    /// Authentication error (invalid API key)
    #[error("Authentication error: {message}")]
    AuthenticationError {
        message: String,
        provider: String,
        suggestion: Option<String>,
    },

    /// Rate limit error (too many requests)
    #[error("Rate limit error: {message}")]
    RateLimitError {
        message: String,
        suggestion: Option<String>,
    },

    /// Provider-specific error (API returned error)
    #[error("Provider error: {message}")]
    ProviderError {
        message: String,
        suggestion: Option<String>,
    },

    /// Request timeout
    #[error("Request timed out")]
    Timeout { suggestion: Option<String> },

    /// No provider available for routing
    #[error("No provider available")]
    NoProviderAvailable { suggestion: Option<String> },

    /// Invalid configuration
    #[error("Invalid configuration: {message}")]
    InvalidConfig {
        message: String,
        suggestion: Option<String>,
    },

    /// Keychain access error
    #[error("Keychain error: {message}")]
    KeychainError {
        message: String,
        suggestion: Option<String>,
    },

    /// Generic error for other cases
    #[error("Aether error: {message}")]
    Other {
        message: String,
        suggestion: Option<String>,
    },

    /// Permission denied error (for Accessibility and Input Monitoring)
    #[error("Permission denied: {message}")]
    PermissionDenied {
        message: String,
        suggestion: Option<String>,
    },
}

impl AetherError {
    /// Create a hotkey error with a message
    pub fn hotkey<S: Into<String>>(msg: S) -> Self {
        AetherError::HotkeyError {
            message: msg.into(),
            suggestion: Some("Please check Accessibility permissions in System Settings → Privacy & Security → Accessibility".to_string()),
        }
    }

    /// Create a clipboard error with a message
    pub fn clipboard<S: Into<String>>(msg: S) -> Self {
        AetherError::ClipboardError {
            message: msg.into(),
            suggestion: Some(
                "Ensure you have copied text or an image before pressing Cmd+~".to_string(),
            ),
        }
    }

    /// Create an input simulation error with a message
    pub fn input_simulation<S: Into<String>>(msg: S) -> Self {
        AetherError::InputSimulationError {
            message: msg.into(),
            suggestion: Some("Grant Accessibility permission in System Settings → Privacy & Security → Accessibility".to_string()),
        }
    }

    /// Create a callback error with a message
    pub fn callback<S: Into<String>>(msg: S) -> Self {
        AetherError::CallbackError {
            message: msg.into(),
            suggestion: Some("This is an internal error. Please restart Aether.".to_string()),
        }
    }

    /// Create a config/database error with a message
    pub fn config<S: Into<String>>(msg: S) -> Self {
        AetherError::ConfigError {
            message: msg.into(),
            suggestion: Some(
                "Check your configuration file at ~/.config/aether/config.toml".to_string(),
            ),
        }
    }

    /// Create a network error with a message
    pub fn network<S: Into<String>>(msg: S) -> Self {
        AetherError::NetworkError {
            message: msg.into(),
            suggestion: Some("Check your internet connection and try again".to_string()),
        }
    }

    /// Create an authentication error with a message and provider
    pub fn authentication<S: Into<String>>(provider: S, msg: S) -> Self {
        let provider_name = provider.into();
        AetherError::AuthenticationError {
            message: msg.into(),
            provider: provider_name.clone(),
            suggestion: Some(format!(
                "Verify your {} API key in Settings → Providers → {}",
                provider_name, provider_name
            )),
        }
    }

    /// Create a rate limit error with a message
    pub fn rate_limit<S: Into<String>>(msg: S) -> Self {
        AetherError::RateLimitError {
            message: msg.into(),
            suggestion: Some("Wait 60 seconds or upgrade your API plan".to_string()),
        }
    }

    /// Create a provider error with a message
    pub fn provider<S: Into<String>>(msg: S) -> Self {
        AetherError::ProviderError {
            message: msg.into(),
            suggestion: Some(
                "Try switching to a different AI provider in Settings → Providers".to_string(),
            ),
        }
    }

    /// Create an invalid config error with a message
    pub fn invalid_config<S: Into<String>>(msg: S) -> Self {
        AetherError::InvalidConfig {
            message: msg.into(),
            suggestion: Some(
                "Edit your configuration in Settings or check ~/.config/aether/config.toml"
                    .to_string(),
            ),
        }
    }

    /// Create a keychain error with a message
    pub fn keychain<S: Into<String>>(msg: S) -> Self {
        AetherError::KeychainError {
            message: msg.into(),
            suggestion: Some("Check Keychain Access permissions in System Settings".to_string()),
        }
    }

    /// Create a generic error with a message
    pub fn other<S: Into<String>>(msg: S) -> Self {
        AetherError::Other {
            message: msg.into(),
            suggestion: None,
        }
    }

    /// Create a permission denied error with a message
    pub fn permission_denied<S: Into<String>>(msg: S) -> Self {
        AetherError::PermissionDenied {
            message: msg.into(),
            suggestion: Some("Grant required permissions in System Settings → Privacy & Security → Accessibility and Input Monitoring".to_string()),
        }
    }

    /// Get the suggestion for this error, if available
    ///
    /// Returns a user-friendly actionable suggestion for how to resolve the error.
    pub fn suggestion(&self) -> Option<&str> {
        match self {
            AetherError::HotkeyError { suggestion, .. }
            | AetherError::ClipboardError { suggestion, .. }
            | AetherError::InputSimulationError { suggestion, .. }
            | AetherError::CallbackError { suggestion, .. }
            | AetherError::ConfigError { suggestion, .. }
            | AetherError::NetworkError { suggestion, .. }
            | AetherError::AuthenticationError { suggestion, .. }
            | AetherError::RateLimitError { suggestion, .. }
            | AetherError::ProviderError { suggestion, .. }
            | AetherError::Timeout { suggestion }
            | AetherError::NoProviderAvailable { suggestion }
            | AetherError::InvalidConfig { suggestion, .. }
            | AetherError::KeychainError { suggestion, .. }
            | AetherError::Other { suggestion, .. }
            | AetherError::PermissionDenied { suggestion, .. } => suggestion.as_deref(),
        }
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
    /// let err = AetherError::authentication("OpenAI", "401 Unauthorized");
    /// assert_eq!(
    ///     err.user_friendly_message(),
    ///     "Authentication failed. Please check your API key in settings."
    /// );
    /// ```
    pub fn user_friendly_message(&self) -> String {
        match self {
            AetherError::AuthenticationError { .. } => {
                "Authentication failed. Please check your API key in settings.".to_string()
            }
            AetherError::RateLimitError { .. } => {
                "Rate limit exceeded. Please try again in a few moments.".to_string()
            }
            AetherError::NetworkError { .. } => {
                "Network connection failed. Please check your internet connection.".to_string()
            }
            AetherError::Timeout { .. } => {
                "Request timed out. The AI service is taking too long to respond. Please try again."
                    .to_string()
            }
            AetherError::NoProviderAvailable { .. } => {
                "No AI provider is configured. Please configure at least one provider in settings."
                    .to_string()
            }
            AetherError::InvalidConfig { message, .. } => {
                format!(
                    "Configuration error: {}. Please check your settings.",
                    message
                )
            }
            AetherError::ProviderError { message, .. } => {
                // Show the actual error message for debugging
                // Previously we hid 5xx errors, but users need to see what went wrong
                format!("AI service error: {}. Please try again.", message)
            }
            AetherError::HotkeyError { message, .. } => {
                format!(
                    "Hotkey error: {}. Please check your system permissions.",
                    message
                )
            }
            AetherError::ClipboardError { message, .. } => {
                format!(
                    "Clipboard error: {}. Please check your system permissions.",
                    message
                )
            }
            AetherError::InputSimulationError { message, .. } => {
                format!(
                    "Input simulation error: {}. Please check accessibility permissions.",
                    message
                )
            }
            AetherError::ConfigError { message, .. } => {
                format!(
                    "Configuration error: {}. Please check your settings file.",
                    message
                )
            }
            AetherError::KeychainError { message, .. } => {
                format!(
                    "Keychain access error: {}. Please check your system permissions.",
                    message
                )
            }
            AetherError::CallbackError { message, .. } => {
                format!(
                    "Internal error: {}. Please restart the application.",
                    message
                )
            }
            AetherError::Other { message, .. } => {
                format!("An error occurred: {}. Please try again.", message)
            }
            AetherError::PermissionDenied { message, .. } => {
                format!(
                    "Permission denied: {}. Please grant required permissions in System Settings.",
                    message
                )
            }
        }
    }
}

/// Type alias for Results using AetherError
pub type Result<T> = std::result::Result<T, AetherError>;

/// Simple exception enum for UniFFI 0.25 compatibility
///
/// UniFFI 0.25 has bugs with [Error] enum when variants have associated data (flat_error issue).
/// This simple unit-variant enum works. Error details are passed via callback before throwing.
#[derive(Debug, Clone, thiserror::Error)]
pub enum AetherException {
    #[error("An error occurred")]
    Error,
}

impl From<AetherError> for AetherException {
    fn from(_error: AetherError) -> Self {
        // Note: Error details should be sent via callback before converting
        // Callers should use the pattern: handler.on_error(msg, suggestion); Err(AetherException::Error)?
        AetherException::Error
    }
}

impl From<String> for AetherException {
    fn from(_message: String) -> Self {
        AetherException::Error
    }
}

impl From<&str> for AetherException {
    fn from(_message: &str) -> Self {
        AetherException::Error
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_hotkey_error_creation() {
        let err = AetherError::hotkey("test error");
        assert!(matches!(err, AetherError::HotkeyError { .. }));
        assert_eq!(err.to_string(), "Hotkey listener error: test error");
        assert!(err.suggestion().is_some());
        assert!(err.suggestion().unwrap().contains("Accessibility"));
    }

    #[test]
    fn test_clipboard_error_creation() {
        let err = AetherError::clipboard("access denied");
        assert!(matches!(err, AetherError::ClipboardError { .. }));
        assert_eq!(err.to_string(), "Clipboard error: access denied");
        assert!(err.suggestion().is_some());
    }

    #[test]
    fn test_callback_error_creation() {
        let err = AetherError::callback("callback failed");
        assert!(matches!(err, AetherError::CallbackError { .. }));
        assert_eq!(err.to_string(), "FFI callback error: callback failed");
        assert!(err.suggestion().is_some());
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
        assert!(matches!(err, AetherError::NetworkError { .. }));
        assert_eq!(err.to_string(), "Network error: connection failed");
        assert!(err.suggestion().is_some());
        assert!(err.suggestion().unwrap().contains("internet"));
    }

    #[test]
    fn test_authentication_error() {
        let err = AetherError::authentication("OpenAI", "invalid API key");
        assert!(matches!(err, AetherError::AuthenticationError { .. }));
        assert_eq!(err.to_string(), "Authentication error: invalid API key");
        assert!(err.suggestion().is_some());
        assert!(err.suggestion().unwrap().contains("OpenAI"));
    }

    #[test]
    fn test_rate_limit_error() {
        let err = AetherError::rate_limit("too many requests");
        assert!(matches!(err, AetherError::RateLimitError { .. }));
        assert_eq!(err.to_string(), "Rate limit error: too many requests");
        assert!(err.suggestion().is_some());
        assert!(err.suggestion().unwrap().contains("60 seconds"));
    }

    #[test]
    fn test_provider_error() {
        let err = AetherError::provider("API returned 500");
        assert!(matches!(err, AetherError::ProviderError { .. }));
        assert_eq!(err.to_string(), "Provider error: API returned 500");
        assert!(err.suggestion().is_some());
    }

    #[test]
    fn test_timeout_error() {
        let err = AetherError::Timeout {
            suggestion: Some("Try again".to_string()),
        };
        assert_eq!(err.to_string(), "Request timed out");
        assert_eq!(err.suggestion(), Some("Try again"));
    }

    #[test]
    fn test_no_provider_available() {
        let err = AetherError::NoProviderAvailable {
            suggestion: Some("Add a provider".to_string()),
        };
        assert_eq!(err.to_string(), "No provider available");
        assert_eq!(err.suggestion(), Some("Add a provider"));
    }

    #[test]
    fn test_invalid_config_error() {
        let err = AetherError::invalid_config("missing API key");
        assert!(matches!(err, AetherError::InvalidConfig { .. }));
        assert_eq!(err.to_string(), "Invalid configuration: missing API key");
        assert!(err.suggestion().is_some());
    }

    #[test]
    fn test_suggestion_method() {
        let err = AetherError::authentication("Claude", "401");
        assert!(err.suggestion().is_some());
        let suggestion = err.suggestion().unwrap();
        assert!(suggestion.contains("Claude"));
        assert!(suggestion.contains("Settings"));
    }

    #[test]
    fn test_user_friendly_message() {
        let err = AetherError::authentication("OpenAI", "401 Unauthorized");
        let msg = err.user_friendly_message();
        assert!(msg.contains("Authentication"));
        assert!(msg.contains("API key"));
    }
}
