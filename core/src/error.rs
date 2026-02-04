/// Custom error types for Aether core library.
///
/// All errors in the Aether core are represented using this enum,
/// which provides clear error messages and integrates with UniFFI
/// for automatic conversion to Swift/Kotlin exceptions.
use thiserror::Error;

/// Safely truncate a string at character boundaries (UTF-8 safe)
fn truncate_str(s: &str, max_chars: usize) -> String {
    if s.chars().count() <= max_chars {
        return s.to_string();
    }
    let end_byte = s
        .char_indices()
        .nth(max_chars)
        .map(|(i, _)| i)
        .unwrap_or(s.len());
    format!("{}...", &s[..end_byte])
}

#[derive(Debug, Error)]
pub enum AlephError {
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

    /// Error occurred when invoking callbacks
    #[error("Callback error: {message}")]
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

    /// Video transcript extraction error
    #[error("Video error: {message}")]
    VideoError {
        message: String,
        suggestion: Option<String>,
    },

    /// File or resource not found
    #[error("Not found: {0}")]
    NotFound(String),

    /// I/O operation error
    #[error("I/O error: {0}")]
    IoError(String),

    /// Git operation error
    #[error("Git error: {0}")]
    GitError(String),

    /// MCP tool not found
    #[error("MCP tool not found: {0}")]
    McpToolNotFound(String),

    /// MCP request timeout
    #[error("MCP request timed out")]
    McpTimeout,

    /// Native tool not found
    #[error("Tool not found: {name}")]
    ToolNotFound {
        name: String,
        suggestion: Option<String>,
    },

    /// Operation was cancelled by user
    #[error("Operation cancelled")]
    Cancelled,

    /// Task requires additional user input to complete
    /// This is returned when an LLM response indicates it cannot proceed
    /// without more information from the user.
    #[error("Task '{task_name}' needs additional input: {message}")]
    MissingInput {
        task_id: String,
        task_name: String,
        message: String,
    },

    /// Runtime manager error (uv, fnm, yt-dlp, etc.)
    #[error("Runtime error [{runtime_id}]: {message}")]
    RuntimeError {
        message: String,
        runtime_id: String,
        suggestion: Option<String>,
    },

    /// Data corruption or integrity error
    #[error("Data corruption: {0}")]
    CorruptData(String),

    /// Channel closed error (internal communication failure)
    #[error("Channel closed: {0}")]
    ChannelClosed(String),
}

impl AlephError {
    /// Create a hotkey error with a message
    pub fn hotkey<S: Into<String>>(msg: S) -> Self {
        AlephError::HotkeyError {
            message: msg.into(),
            suggestion: Some("Please check Accessibility permissions in System Settings → Privacy & Security → Accessibility".to_string()),
        }
    }

    /// Create a clipboard error with a message
    pub fn clipboard<S: Into<String>>(msg: S) -> Self {
        AlephError::ClipboardError {
            message: msg.into(),
            suggestion: Some(
                "Ensure you have copied text or an image before pressing Cmd+~".to_string(),
            ),
        }
    }

    /// Create an input simulation error with a message
    pub fn input_simulation<S: Into<String>>(msg: S) -> Self {
        AlephError::InputSimulationError {
            message: msg.into(),
            suggestion: Some("Grant Accessibility permission in System Settings → Privacy & Security → Accessibility".to_string()),
        }
    }

    /// Create a callback error with a message
    pub fn callback<S: Into<String>>(msg: S) -> Self {
        AlephError::CallbackError {
            message: msg.into(),
            suggestion: Some("This is an internal error. Please restart Aether.".to_string()),
        }
    }

    /// Create a config/database error with a message
    pub fn config<S: Into<String>>(msg: S) -> Self {
        AlephError::ConfigError {
            message: msg.into(),
            suggestion: Some(
                "Check your configuration file at ~/.aleph/config.toml".to_string(),
            ),
        }
    }

    /// Create a network error with a message
    pub fn network<S: Into<String>>(msg: S) -> Self {
        AlephError::NetworkError {
            message: msg.into(),
            suggestion: Some("Check your internet connection and try again".to_string()),
        }
    }

    /// Create an authentication error with a message and provider
    pub fn authentication<S: Into<String>>(provider: S, msg: S) -> Self {
        let provider_name = provider.into();
        AlephError::AuthenticationError {
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
        AlephError::RateLimitError {
            message: msg.into(),
            suggestion: Some("Wait 60 seconds or upgrade your API plan".to_string()),
        }
    }

    /// Create a provider error with a message
    pub fn provider<S: Into<String>>(msg: S) -> Self {
        AlephError::ProviderError {
            message: msg.into(),
            suggestion: Some(
                "Try switching to a different AI provider in Settings → Providers".to_string(),
            ),
        }
    }

    /// Create an invalid config error with a message
    pub fn invalid_config<S: Into<String>>(msg: S) -> Self {
        AlephError::InvalidConfig {
            message: msg.into(),
            suggestion: Some(
                "Edit your configuration in Settings or check ~/.aleph/config.toml".to_string(),
            ),
        }
    }

    /// Create a keychain error with a message
    pub fn keychain<S: Into<String>>(msg: S) -> Self {
        AlephError::KeychainError {
            message: msg.into(),
            suggestion: Some("Check Keychain Access permissions in System Settings".to_string()),
        }
    }

    /// Create a generic error with a message
    pub fn other<S: Into<String>>(msg: S) -> Self {
        AlephError::Other {
            message: msg.into(),
            suggestion: None,
        }
    }

    /// Create a permission denied error with a message
    pub fn permission_denied<S: Into<String>>(msg: S) -> Self {
        AlephError::PermissionDenied {
            message: msg.into(),
            suggestion: Some("Grant required permissions in System Settings → Privacy & Security → Accessibility and Input Monitoring".to_string()),
        }
    }

    /// Create a video transcript extraction error with a message
    pub fn video<S: Into<String>>(msg: S) -> Self {
        AlephError::VideoError {
            message: msg.into(),
            suggestion: Some("Check if the video has captions available. Try a different video or ensure you have internet connectivity.".to_string()),
        }
    }

    /// Create a video transcript extraction error with a custom suggestion
    pub fn video_with_suggestion<S: Into<String>, T: Into<String>>(msg: S, suggestion: T) -> Self {
        AlephError::VideoError {
            message: msg.into(),
            suggestion: Some(suggestion.into()),
        }
    }

    /// Get the suggestion for this error, if available
    ///
    /// Returns a user-friendly actionable suggestion for how to resolve the error.
    pub fn suggestion(&self) -> Option<&str> {
        match self {
            AlephError::HotkeyError { suggestion, .. }
            | AlephError::ClipboardError { suggestion, .. }
            | AlephError::InputSimulationError { suggestion, .. }
            | AlephError::CallbackError { suggestion, .. }
            | AlephError::ConfigError { suggestion, .. }
            | AlephError::NetworkError { suggestion, .. }
            | AlephError::AuthenticationError { suggestion, .. }
            | AlephError::RateLimitError { suggestion, .. }
            | AlephError::ProviderError { suggestion, .. }
            | AlephError::Timeout { suggestion }
            | AlephError::NoProviderAvailable { suggestion }
            | AlephError::InvalidConfig { suggestion, .. }
            | AlephError::KeychainError { suggestion, .. }
            | AlephError::Other { suggestion, .. }
            | AlephError::PermissionDenied { suggestion, .. }
            | AlephError::VideoError { suggestion, .. }
            | AlephError::ToolNotFound { suggestion, .. }
            | AlephError::RuntimeError { suggestion, .. } => suggestion.as_deref(),
            // Simple error types without suggestion field
            AlephError::NotFound(_)
            | AlephError::IoError(_)
            | AlephError::GitError(_)
            | AlephError::McpToolNotFound(_)
            | AlephError::McpTimeout
            | AlephError::Cancelled
            | AlephError::MissingInput { .. }
            | AlephError::CorruptData(_)
            | AlephError::ChannelClosed(_) => None,
        }
    }

    /// Get a user-friendly error message suitable for display in the UI
    ///
    /// This method converts technical error messages into friendly,
    /// actionable messages that users can understand and act upon.
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// use alephcore::error::AlephError;
    ///
    /// let err = AlephError::authentication("OpenAI", "401 Unauthorized");
    /// assert_eq!(
    ///     err.user_friendly_message(),
    ///     "Authentication failed. Please check your API key in settings."
    /// );
    /// ```
    pub fn user_friendly_message(&self) -> String {
        match self {
            AlephError::AuthenticationError { .. } => {
                "Authentication failed. Please check your API key in settings.".to_string()
            }
            AlephError::RateLimitError { .. } => {
                "Rate limit exceeded. Please try again in a few moments.".to_string()
            }
            AlephError::NetworkError { .. } => {
                "Network connection failed. Please check your internet connection.".to_string()
            }
            AlephError::Timeout { .. } => {
                "Request timed out. The AI service is taking too long to respond. Please try again."
                    .to_string()
            }
            AlephError::NoProviderAvailable { .. } => {
                "No AI provider is configured. Please configure at least one provider in settings."
                    .to_string()
            }
            AlephError::InvalidConfig { message, .. } => {
                format!(
                    "Configuration error: {}. Please check your settings.",
                    message
                )
            }
            AlephError::ProviderError { message, .. } => {
                // Show the actual error message for debugging
                // Previously we hid 5xx errors, but users need to see what went wrong
                format!("AI service error: {}. Please try again.", message)
            }
            AlephError::HotkeyError { message, .. } => {
                format!(
                    "Hotkey error: {}. Please check your system permissions.",
                    message
                )
            }
            AlephError::ClipboardError { message, .. } => {
                format!(
                    "Clipboard error: {}. Please check your system permissions.",
                    message
                )
            }
            AlephError::InputSimulationError { message, .. } => {
                format!(
                    "Input simulation error: {}. Please check accessibility permissions.",
                    message
                )
            }
            AlephError::ConfigError { message, .. } => {
                format!(
                    "Configuration error: {}. Please check your settings file.",
                    message
                )
            }
            AlephError::KeychainError { message, .. } => {
                format!(
                    "Keychain access error: {}. Please check your system permissions.",
                    message
                )
            }
            AlephError::CallbackError { message, .. } => {
                format!(
                    "Internal error: {}. Please restart the application.",
                    message
                )
            }
            AlephError::Other { message, .. } => {
                format!("An error occurred: {}. Please try again.", message)
            }
            AlephError::PermissionDenied { message, .. } => {
                format!(
                    "Permission denied: {}. Please grant required permissions in System Settings.",
                    message
                )
            }
            AlephError::VideoError { message, .. } => {
                format!(
                    "Video processing error: {}. Check if the video has captions available.",
                    message
                )
            }
            AlephError::NotFound(path) => {
                format!("File or resource not found: {}", path)
            }
            AlephError::IoError(msg) => {
                format!("I/O error: {}", msg)
            }
            AlephError::GitError(msg) => {
                format!("Git operation failed: {}", msg)
            }
            AlephError::McpToolNotFound(tool) => {
                format!("MCP tool '{}' not found", tool)
            }
            AlephError::McpTimeout => "MCP request timed out. Please try again.".to_string(),
            AlephError::ToolNotFound { name, suggestion } => {
                if let Some(sug) = suggestion {
                    format!("Tool '{}' not found. {}", name, sug)
                } else {
                    format!("Tool '{}' not found", name)
                }
            }
            AlephError::Cancelled => "Operation cancelled.".to_string(),
            AlephError::RuntimeError {
                message,
                runtime_id,
                ..
            } => {
                format!(
                    "Runtime '{}' error: {}. Check Settings → Runtimes for details.",
                    runtime_id, message
                )
            }
            AlephError::MissingInput {
                task_name,
                message,
                ..
            } => {
                format!(
                    "任务 '{}' 需要更多信息才能继续执行。请提供所需内容后重试。\n详情: {}",
                    task_name,
                    // Truncate message if too long (UTF-8 safe)
                    truncate_str(message, 100)
                )
            }
            AlephError::CorruptData(msg) => {
                format!("Data corruption detected: {}. Please try again or restore from backup.", msg)
            }
            AlephError::ChannelClosed(msg) => {
                format!("Internal communication failed: {}. Please restart the application.", msg)
            }
        }
    }

    /// Create a generic tool error
    pub fn tool<S: Into<String>>(msg: S) -> Self {
        AlephError::Other {
            message: msg.into(),
            suggestion: None,
        }
    }

    /// Create a tool not found error
    pub fn tool_not_found<S: Into<String>>(name: S) -> Self {
        AlephError::ToolNotFound {
            name: name.into(),
            suggestion: None,
        }
    }

    /// Create a tool not found error with suggestion
    pub fn tool_not_found_with_suggestion<S: Into<String>, T: Into<String>>(
        name: S,
        suggestion: T,
    ) -> Self {
        AlephError::ToolNotFound {
            name: name.into(),
            suggestion: Some(suggestion.into()),
        }
    }

    /// Create an invalid input error
    ///
    /// Used when user input (IDs, parameters, etc.) fails validation.
    pub fn invalid_input<S: Into<String>>(msg: S) -> Self {
        AlephError::InvalidConfig {
            message: msg.into(),
            suggestion: Some("Please check the input values and try again".to_string()),
        }
    }

    /// Create a cancelled error
    ///
    /// Used when an operation is cancelled by the user via CancellationToken.
    pub fn cancelled() -> Self {
        AlephError::Cancelled
    }

    /// Create a runtime error with a message
    pub fn runtime<S: Into<String>, M: Into<String>>(runtime_id: S, msg: M) -> Self {
        AlephError::RuntimeError {
            message: msg.into(),
            runtime_id: runtime_id.into(),
            suggestion: Some("Check your network connection and try again. If the problem persists, try manually installing the runtime.".to_string()),
        }
    }

    /// Create a runtime error with a custom suggestion
    pub fn runtime_with_suggestion<S: Into<String>, M: Into<String>, T: Into<String>>(
        runtime_id: S,
        msg: M,
        suggestion: T,
    ) -> Self {
        AlephError::RuntimeError {
            message: msg.into(),
            runtime_id: runtime_id.into(),
            suggestion: Some(suggestion.into()),
        }
    }

    /// Create a channel closed error
    ///
    /// Used when an internal communication channel is unexpectedly closed.
    pub fn channel_closed<S: Into<String>>(msg: S) -> Self {
        AlephError::ChannelClosed(msg.into())
    }
}

/// Type alias for Results using AlephError
pub type Result<T> = std::result::Result<T, AlephError>;

impl From<serde_json::Error> for AlephError {
    fn from(err: serde_json::Error) -> Self {
        AlephError::IoError(format!("JSON serialization error: {}", err))
    }
}

impl From<std::io::Error> for AlephError {
    fn from(err: std::io::Error) -> Self {
        AlephError::IoError(err.to_string())
    }
}

/// Simple exception enum for UniFFI 0.25 compatibility
///
/// UniFFI 0.25 has bugs with [Error] enum when variants have associated data (flat_error issue).
/// This simple unit-variant enum works. Error details are passed via callback before throwing.
#[derive(Debug, Clone, thiserror::Error)]
pub enum AlephException {
    #[error("An error occurred")]
    Error,
}

impl From<AlephError> for AlephException {
    fn from(_error: AlephError) -> Self {
        // Note: Error details should be sent via callback before converting
        // Callers should use the pattern: handler.on_error(msg, suggestion); Err(AlephException::Error)?
        AlephException::Error
    }
}

impl From<String> for AlephException {
    fn from(_message: String) -> Self {
        AlephException::Error
    }
}

impl From<&str> for AlephException {
    fn from(_message: &str) -> Self {
        AlephException::Error
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_hotkey_error_creation() {
        let err = AlephError::hotkey("test error");
        assert!(matches!(err, AlephError::HotkeyError { .. }));
        assert_eq!(err.to_string(), "Hotkey listener error: test error");
        assert!(err.suggestion().is_some());
        assert!(err.suggestion().unwrap().contains("Accessibility"));
    }

    #[test]
    fn test_clipboard_error_creation() {
        let err = AlephError::clipboard("access denied");
        assert!(matches!(err, AlephError::ClipboardError { .. }));
        assert_eq!(err.to_string(), "Clipboard error: access denied");
        assert!(err.suggestion().is_some());
    }

    #[test]
    fn test_callback_error_creation() {
        let err = AlephError::callback("callback failed");
        assert!(matches!(err, AlephError::CallbackError { .. }));
        assert_eq!(err.to_string(), "Callback error: callback failed");
        assert!(err.suggestion().is_some());
    }

    #[test]
    fn test_error_display() {
        let err = AlephError::other("generic error");
        let display = format!("{}", err);
        assert_eq!(display, "Aether error: generic error");
    }

    #[test]
    fn test_error_debug() {
        let err = AlephError::hotkey("test");
        let debug = format!("{:?}", err);
        assert!(debug.contains("HotkeyError"));
    }

    #[test]
    fn test_network_error() {
        let err = AlephError::network("connection failed");
        assert!(matches!(err, AlephError::NetworkError { .. }));
        assert_eq!(err.to_string(), "Network error: connection failed");
        assert!(err.suggestion().is_some());
        assert!(err.suggestion().unwrap().contains("internet"));
    }

    #[test]
    fn test_authentication_error() {
        let err = AlephError::authentication("OpenAI", "invalid API key");
        assert!(matches!(err, AlephError::AuthenticationError { .. }));
        assert_eq!(err.to_string(), "Authentication error: invalid API key");
        assert!(err.suggestion().is_some());
        assert!(err.suggestion().unwrap().contains("OpenAI"));
    }

    #[test]
    fn test_rate_limit_error() {
        let err = AlephError::rate_limit("too many requests");
        assert!(matches!(err, AlephError::RateLimitError { .. }));
        assert_eq!(err.to_string(), "Rate limit error: too many requests");
        assert!(err.suggestion().is_some());
        assert!(err.suggestion().unwrap().contains("60 seconds"));
    }

    #[test]
    fn test_provider_error() {
        let err = AlephError::provider("API returned 500");
        assert!(matches!(err, AlephError::ProviderError { .. }));
        assert_eq!(err.to_string(), "Provider error: API returned 500");
        assert!(err.suggestion().is_some());
    }

    #[test]
    fn test_timeout_error() {
        let err = AlephError::Timeout {
            suggestion: Some("Try again".to_string()),
        };
        assert_eq!(err.to_string(), "Request timed out");
        assert_eq!(err.suggestion(), Some("Try again"));
    }

    #[test]
    fn test_no_provider_available() {
        let err = AlephError::NoProviderAvailable {
            suggestion: Some("Add a provider".to_string()),
        };
        assert_eq!(err.to_string(), "No provider available");
        assert_eq!(err.suggestion(), Some("Add a provider"));
    }

    #[test]
    fn test_invalid_config_error() {
        let err = AlephError::invalid_config("missing API key");
        assert!(matches!(err, AlephError::InvalidConfig { .. }));
        assert_eq!(err.to_string(), "Invalid configuration: missing API key");
        assert!(err.suggestion().is_some());
    }

    #[test]
    fn test_suggestion_method() {
        let err = AlephError::authentication("Claude", "401");
        assert!(err.suggestion().is_some());
        let suggestion = err.suggestion().unwrap();
        assert!(suggestion.contains("Claude"));
        assert!(suggestion.contains("Settings"));
    }

    #[test]
    fn test_user_friendly_message() {
        let err = AlephError::authentication("OpenAI", "401 Unauthorized");
        let msg = err.user_friendly_message();
        assert!(msg.contains("Authentication"));
        assert!(msg.contains("API key"));
    }
}
