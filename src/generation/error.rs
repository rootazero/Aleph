/// Error types for the media generation module
///
/// This module defines all error types that can occur during generation operations.
/// Errors are categorized to help with retry logic, user feedback, and fallback decisions.
///
/// # Error Categories
///
/// - **Retryable**: Temporary failures that may succeed on retry (rate limits, timeouts)
/// - **User Action Required**: Errors that need user intervention (invalid API key, quota exceeded)
/// - **Fallback Recommended**: Errors where switching providers may help (unsupported feature)
use crate::error::AlephError;
use std::time::Duration;
use thiserror::Error;

/// Errors that can occur during media generation operations
///
/// # Example
///
/// ```rust
/// use alephcore::generation::GenerationError;
/// use std::time::Duration;
///
/// let error = GenerationError::rate_limit("Too many requests", Some(Duration::from_secs(60)));
///
/// assert!(error.is_retryable());
/// assert!(!error.needs_user_action());
/// ```
#[derive(Debug, Error)]
pub enum GenerationError {
    /// Invalid or missing API key
    #[error("Authentication failed: {message}")]
    AuthenticationError { message: String, provider: String },

    /// Rate limit exceeded
    #[error("Rate limit exceeded: {message}")]
    RateLimitError {
        message: String,
        /// Time to wait before retrying
        retry_after: Option<Duration>,
    },

    /// Usage quota exceeded (monthly/daily limits)
    #[error("Quota exceeded: {message}")]
    QuotaExceededError {
        message: String,
        /// When the quota resets
        resets_at: Option<String>,
    },

    /// Request timed out
    #[error("Generation timed out after {duration:?}")]
    TimeoutError { duration: Duration },

    /// Network connectivity error
    #[error("Network error: {message}")]
    NetworkError { message: String },

    /// Invalid request parameters
    #[error("Invalid parameters: {message}")]
    InvalidParametersError {
        message: String,
        /// Which parameter is invalid
        parameter: Option<String>,
    },

    /// Content was rejected by safety filters
    #[error("Content filtered: {message}")]
    ContentFilteredError {
        message: String,
        /// Category of content that was filtered
        category: Option<String>,
    },

    /// Requested feature not supported by provider
    #[error("Feature not supported: {message}")]
    UnsupportedFeatureError {
        message: String,
        /// The feature that is not supported
        feature: String,
        /// Provider that doesn't support it
        provider: String,
    },

    /// Provider returned an error response
    #[error("Provider error: {message}")]
    ProviderError {
        message: String,
        /// HTTP status code if available
        status_code: Option<u16>,
        /// Provider name
        provider: String,
    },

    /// Generation was cancelled
    #[error("Generation cancelled")]
    Cancelled,

    /// Internal processing error
    #[error("Internal error: {message}")]
    InternalError { message: String },

    /// Model not found or not available
    #[error("Model not found: {model}")]
    ModelNotFoundError { model: String, provider: String },

    /// Generation type not supported by provider
    #[error("Generation type {generation_type} not supported by {provider}")]
    UnsupportedGenerationTypeError {
        generation_type: String,
        provider: String,
    },

    /// Output format not supported
    #[error("Output format '{format}' not supported")]
    UnsupportedFormatError {
        format: String,
        /// Supported formats for reference
        supported: Vec<String>,
    },

    /// Size/dimension not supported
    #[error("Dimension not supported: {message}")]
    UnsupportedDimensionError {
        message: String,
        /// Suggested dimensions
        suggested: Option<String>,
    },

    /// Async generation job failed
    #[error("Generation job failed: {message}")]
    JobFailedError {
        message: String,
        job_id: Option<String>,
    },

    /// Failed to download generated content
    #[error("Download failed: {message}")]
    DownloadError {
        message: String,
        url: Option<String>,
    },

    /// Serialization/deserialization error
    #[error("Serialization error: {message}")]
    SerializationError { message: String },

    /// A provider with this name is already registered in the registry.
    ///
    /// This is a configuration-level error (a duplicate
    /// `[generation.<name>]` section), NOT an internal failure — mapping it
    /// to `InternalError` used to tell the user to "try again" for a bug
    /// that retrying can never fix.
    #[error("Provider '{name}' is already registered")]
    DuplicateProvider { name: String },
}

impl GenerationError {
    // === Factory methods ===

    /// Create an authentication error
    pub fn authentication<S: Into<String>, P: Into<String>>(message: S, provider: P) -> Self {
        Self::AuthenticationError {
            message: message.into(),
            provider: provider.into(),
        }
    }

    /// Create a rate limit error
    pub fn rate_limit<S: Into<String>>(message: S, retry_after: Option<Duration>) -> Self {
        Self::RateLimitError {
            message: message.into(),
            retry_after,
        }
    }

    /// Create a quota exceeded error
    pub fn quota_exceeded<S: Into<String>>(message: S, resets_at: Option<String>) -> Self {
        Self::QuotaExceededError {
            message: message.into(),
            resets_at,
        }
    }

    /// Create a timeout error
    #[must_use]
    pub const fn timeout(duration: Duration) -> Self {
        Self::TimeoutError { duration }
    }

    /// Create a network error
    pub fn network<S: Into<String>>(message: S) -> Self {
        Self::NetworkError {
            message: message.into(),
        }
    }

    /// Create an invalid parameters error
    pub fn invalid_parameters<S: Into<String>>(message: S, parameter: Option<String>) -> Self {
        Self::InvalidParametersError {
            message: message.into(),
            parameter,
        }
    }

    /// Create a content filtered error
    pub fn content_filtered<S: Into<String>>(message: S, category: Option<String>) -> Self {
        Self::ContentFilteredError {
            message: message.into(),
            category,
        }
    }

    /// Create an unsupported feature error
    pub fn unsupported_feature<S: Into<String>, F: Into<String>, P: Into<String>>(
        message: S,
        feature: F,
        provider: P,
    ) -> Self {
        Self::UnsupportedFeatureError {
            message: message.into(),
            feature: feature.into(),
            provider: provider.into(),
        }
    }

    /// Create a provider error
    pub fn provider<S: Into<String>, P: Into<String>>(
        message: S,
        status_code: Option<u16>,
        provider: P,
    ) -> Self {
        Self::ProviderError {
            message: message.into(),
            status_code,
            provider: provider.into(),
        }
    }

    /// Create a cancelled error
    #[must_use]
    pub const fn cancelled() -> Self {
        Self::Cancelled
    }

    /// Create an internal error
    pub fn internal<S: Into<String>>(message: S) -> Self {
        Self::InternalError {
            message: message.into(),
        }
    }

    /// Create a model not found error
    pub fn model_not_found<M: Into<String>, P: Into<String>>(model: M, provider: P) -> Self {
        Self::ModelNotFoundError {
            model: model.into(),
            provider: provider.into(),
        }
    }

    /// Create an unsupported generation type error
    pub fn unsupported_generation_type<G: Into<String>, P: Into<String>>(
        generation_type: G,
        provider: P,
    ) -> Self {
        Self::UnsupportedGenerationTypeError {
            generation_type: generation_type.into(),
            provider: provider.into(),
        }
    }

    /// Create an unsupported format error
    pub fn unsupported_format<F: Into<String>>(format: F, supported: Vec<String>) -> Self {
        Self::UnsupportedFormatError {
            format: format.into(),
            supported,
        }
    }

    /// Create an unsupported dimension error
    pub fn unsupported_dimension<S: Into<String>>(message: S, suggested: Option<String>) -> Self {
        Self::UnsupportedDimensionError {
            message: message.into(),
            suggested,
        }
    }

    /// Create a job failed error
    pub fn job_failed<S: Into<String>>(message: S, job_id: Option<String>) -> Self {
        Self::JobFailedError {
            message: message.into(),
            job_id,
        }
    }

    /// Create a download error
    pub fn download<S: Into<String>>(message: S, url: Option<String>) -> Self {
        Self::DownloadError {
            message: message.into(),
            url,
        }
    }

    /// Create a serialization error
    pub fn serialization<S: Into<String>>(message: S) -> Self {
        Self::SerializationError {
            message: message.into(),
        }
    }

    // === Classification methods ===

    /// Check if this error is retryable
    ///
    /// Retryable errors are temporary failures that may succeed on retry.
    /// These include rate limits, timeouts, and transient network issues.
    ///
    /// # Example
    ///
    /// ```rust
    /// use alephcore::generation::GenerationError;
    /// use std::time::Duration;
    ///
    /// let rate_limit = GenerationError::rate_limit("Too many requests", None);
    /// assert!(rate_limit.is_retryable());
    ///
    /// let auth_error = GenerationError::authentication("Invalid key", "openai");
    /// assert!(!auth_error.is_retryable());
    /// ```
    #[must_use]
    pub const fn is_retryable(&self) -> bool {
        matches!(
            self,
            Self::RateLimitError { .. }
                | Self::TimeoutError { .. }
                | Self::NetworkError { .. }
                | Self::ProviderError {
                    status_code: Some(500..=599),
                    ..
                }
                | Self::ProviderError {
                    status_code: Some(429),
                    ..
                }
                | Self::DownloadError { .. }
        )
    }

    /// Check if this error requires user action to resolve
    ///
    /// These errors cannot be fixed automatically and require the user
    /// to update settings, add credits, or modify their request.
    ///
    /// # Example
    ///
    /// ```rust
    /// use alephcore::generation::GenerationError;
    ///
    /// let auth_error = GenerationError::authentication("Invalid key", "openai");
    /// assert!(auth_error.needs_user_action());
    ///
    /// let timeout = GenerationError::timeout(std::time::Duration::from_secs(30));
    /// assert!(!timeout.needs_user_action());
    /// ```
    #[must_use]
    pub const fn needs_user_action(&self) -> bool {
        matches!(
            self,
            Self::AuthenticationError { .. }
                | Self::QuotaExceededError { .. }
                | Self::InvalidParametersError { .. }
                | Self::ContentFilteredError { .. }
                | Self::ModelNotFoundError { .. }
        )
    }

    /// Check if switching to a different provider might help
    ///
    /// These errors indicate that the current provider cannot handle
    /// the request, but another provider might be able to.
    ///
    /// # Example
    ///
    /// ```rust
    /// use alephcore::generation::GenerationError;
    ///
    /// let unsupported = GenerationError::unsupported_feature(
    ///     "Video generation not available",
    ///     "video",
    ///     "dall-e",
    /// );
    /// assert!(unsupported.should_fallback());
    ///
    /// let auth = GenerationError::authentication("Invalid key", "openai");
    /// assert!(!auth.should_fallback());
    /// ```
    #[must_use]
    pub const fn should_fallback(&self) -> bool {
        matches!(
            self,
            Self::UnsupportedFeatureError { .. }
                | Self::UnsupportedGenerationTypeError { .. }
                | Self::UnsupportedFormatError { .. }
                | Self::UnsupportedDimensionError { .. }
                | Self::ModelNotFoundError { .. }
        )
    }

    /// Get the suggested retry delay if available
    ///
    /// Returns the recommended wait time before retrying,
    /// primarily used for rate limit errors.
    ///
    /// # Example
    ///
    /// ```rust
    /// use alephcore::generation::GenerationError;
    /// use std::time::Duration;
    ///
    /// let error = GenerationError::rate_limit(
    ///     "Too many requests",
    ///     Some(Duration::from_secs(60))
    /// );
    ///
    /// assert_eq!(error.retry_after(), Some(Duration::from_secs(60)));
    /// ```
    #[must_use]
    pub const fn retry_after(&self) -> Option<Duration> {
        match self {
            Self::RateLimitError { retry_after, .. } => *retry_after,
            _ => None,
        }
    }

    /// Get the provider name if available
    #[must_use]
    pub fn provider_name(&self) -> Option<&str> {
        match self {
            Self::AuthenticationError { provider, .. } => Some(provider),
            Self::UnsupportedFeatureError { provider, .. } => Some(provider),
            Self::ProviderError { provider, .. } => Some(provider),
            Self::ModelNotFoundError { provider, .. } => Some(provider),
            Self::UnsupportedGenerationTypeError { provider, .. } => Some(provider),
            _ => None,
        }
    }

    /// Get a user-friendly error message
    ///
    /// Returns a message suitable for display in the UI,
    /// with actionable suggestions where applicable.
    #[must_use]
    pub fn user_friendly_message(&self) -> String {
        match self {
            Self::AuthenticationError { provider, .. } => {
                format!(
                    "Authentication failed for {provider}. Please check your API key in settings."
                )
            }
            Self::RateLimitError { retry_after, .. } => {
                if let Some(duration) = retry_after {
                    format!(
                        "Rate limit exceeded. Please wait {} seconds before trying again.",
                        duration.as_secs()
                    )
                } else {
                    "Rate limit exceeded. Please wait a moment before trying again.".to_string()
                }
            }
            Self::QuotaExceededError { resets_at, .. } => {
                if let Some(reset) = resets_at {
                    format!(
                        "Usage quota exceeded. Your quota resets at {reset}. Consider upgrading your plan."
                    )
                } else {
                    "Usage quota exceeded. Consider upgrading your plan or waiting for quota reset."
                        .to_string()
                }
            }
            Self::TimeoutError { duration } => {
                format!(
                    "Generation timed out after {} seconds. Try a simpler prompt or smaller output.",
                    duration.as_secs()
                )
            }
            Self::NetworkError { .. } => {
                "Network error. Please check your internet connection and try again.".to_string()
            }
            Self::InvalidParametersError { parameter, message } => {
                if let Some(param) = parameter {
                    format!("Invalid parameter '{param}': {message}. Please adjust your settings.")
                } else {
                    format!("Invalid parameters: {message}. Please check your request.")
                }
            }
            Self::ContentFilteredError { category, .. } => {
                if let Some(cat) = category {
                    format!("Content was filtered for '{cat}' category. Please modify your prompt.")
                } else {
                    "Content was filtered by safety systems. Please modify your prompt.".to_string()
                }
            }
            Self::UnsupportedFeatureError {
                feature, provider, ..
            } => {
                format!(
                    "The feature '{feature}' is not supported by {provider}. Try a different provider."
                )
            }
            Self::ProviderError {
                message,
                status_code,
                provider,
            } => {
                if let Some(code) = status_code {
                    format!("{provider} returned error {code}: {message}")
                } else {
                    format!("{provider} error: {message}")
                }
            }
            Self::Cancelled => "Generation was cancelled.".to_string(),
            Self::InternalError { message } => {
                format!("Internal error: {message}. Please try again.")
            }
            Self::ModelNotFoundError { model, provider } => {
                format!(
                    "Model '{model}' not found on {provider}. Check the model name or try a different model."
                )
            }
            Self::UnsupportedGenerationTypeError {
                generation_type,
                provider,
            } => {
                format!(
                    "{provider} does not support {generation_type} generation. Try a different provider."
                )
            }
            Self::UnsupportedFormatError { format, supported } => {
                format!(
                    "Output format '{}' is not supported. Supported formats: {}",
                    format,
                    supported.join(", ")
                )
            }
            Self::UnsupportedDimensionError { message, suggested } => {
                if let Some(sug) = suggested {
                    format!("{message}. Suggested: {sug}")
                } else {
                    message.clone()
                }
            }
            Self::JobFailedError { message, job_id } => {
                if let Some(id) = job_id {
                    format!("Generation job {id} failed: {message}")
                } else {
                    format!("Generation job failed: {message}")
                }
            }
            Self::DownloadError { message, url } => {
                if let Some(u) = url {
                    format!("Failed to download from {u}: {message}")
                } else {
                    format!("Download failed: {message}")
                }
            }
            Self::SerializationError { message } => {
                format!("Data processing error: {message}. Please try again.")
            }
            Self::DuplicateProvider { name } => {
                format!(
                    "Provider '{name}' is registered twice. Remove the duplicate \
                     [generation.{name}] section from your config and restart."
                )
            }
        }
    }
}

/// Convert `GenerationError` to `AlephError` for integration with core error handling
impl From<GenerationError> for AlephError {
    fn from(err: GenerationError) -> Self {
        match err {
            GenerationError::AuthenticationError { message, provider } => {
                Self::authentication(provider, message)
            }
            GenerationError::RateLimitError { message, .. } => Self::rate_limit(message),
            GenerationError::QuotaExceededError { message, .. } => {
                Self::rate_limit(format!("Quota exceeded: {message}"))
            }
            GenerationError::TimeoutError { duration } => Self::Timeout {
                suggestion: Some(format!(
                    "Generation timed out after {} seconds. Try a simpler request.",
                    duration.as_secs()
                )),
            },
            GenerationError::NetworkError { message } => Self::network(message),
            GenerationError::InvalidParametersError { message, .. } => {
                Self::invalid_config(message)
            }
            GenerationError::ContentFilteredError { message, .. } => {
                Self::provider(format!("Content filtered: {message}"))
            }
            GenerationError::UnsupportedFeatureError { message, .. } => Self::provider(message),
            GenerationError::ProviderError { message, .. } => Self::provider(message),
            GenerationError::Cancelled => Self::cancelled(),
            GenerationError::InternalError { message } => Self::other(message),
            GenerationError::ModelNotFoundError { model, provider } => {
                Self::invalid_config(format!("Model '{model}' not found on {provider}"))
            }
            GenerationError::UnsupportedGenerationTypeError {
                generation_type,
                provider,
            } => Self::provider(format!(
                "{provider} does not support {generation_type} generation"
            )),
            GenerationError::UnsupportedFormatError { format, .. } => {
                Self::invalid_config(format!("Unsupported format: {format}"))
            }
            GenerationError::UnsupportedDimensionError { message, .. } => {
                Self::invalid_config(message)
            }
            GenerationError::JobFailedError { message, .. } => Self::provider(message),
            GenerationError::DownloadError { message, .. } => Self::network(message),
            GenerationError::SerializationError { message } => Self::IoError(message),
            GenerationError::DuplicateProvider { name } => Self::invalid_config(format!(
                "Duplicate generation provider '{name}' — remove the duplicate \
                 [generation.{name}] section from your config"
            )),
        }
    }
}

/// Result type alias for generation operations
pub type GenerationResult<T> = std::result::Result<T, GenerationError>;

#[cfg(test)]
mod tests {
    use super::*;

    // === Factory method tests ===

    #[test]
    fn test_authentication_error() {
        let err = GenerationError::authentication("Invalid API key", "openai");

        assert!(matches!(err, GenerationError::AuthenticationError { .. }));
        assert!(err.to_string().contains("Authentication failed"));
        assert!(err.to_string().contains("Invalid API key"));
    }

    #[test]
    fn test_rate_limit_error() {
        let err = GenerationError::rate_limit("Too many requests", Some(Duration::from_secs(60)));

        assert!(matches!(err, GenerationError::RateLimitError { .. }));
        assert_eq!(err.retry_after(), Some(Duration::from_secs(60)));
    }

    #[test]
    fn test_timeout_error() {
        let err = GenerationError::timeout(Duration::from_secs(30));

        assert!(matches!(err, GenerationError::TimeoutError { .. }));
        assert!(err.to_string().contains("30"));
    }

    #[test]
    fn test_network_error() {
        let err = GenerationError::network("Connection refused");

        assert!(matches!(err, GenerationError::NetworkError { .. }));
        assert!(err.to_string().contains("Connection refused"));
    }

    #[test]
    fn test_content_filtered_error() {
        let err = GenerationError::content_filtered(
            "Prompt contained inappropriate content",
            Some("violence".to_string()),
        );

        assert!(matches!(err, GenerationError::ContentFilteredError { .. }));
    }

    #[test]
    fn test_unsupported_feature_error() {
        let err = GenerationError::unsupported_feature(
            "4K resolution not available",
            "4k_resolution",
            "dall-e-3",
        );

        assert!(matches!(
            err,
            GenerationError::UnsupportedFeatureError { .. }
        ));
        assert_eq!(err.provider_name(), Some("dall-e-3"));
    }

    #[test]
    fn test_provider_error() {
        let err = GenerationError::provider("Internal server error", Some(500), "openai");

        assert!(matches!(err, GenerationError::ProviderError { .. }));
        assert_eq!(err.provider_name(), Some("openai"));
    }

    #[test]
    fn test_model_not_found_error() {
        let err = GenerationError::model_not_found("dall-e-4", "openai");

        assert!(matches!(err, GenerationError::ModelNotFoundError { .. }));
        assert_eq!(err.provider_name(), Some("openai"));
    }

    #[test]
    fn test_unsupported_format_error() {
        let err =
            GenerationError::unsupported_format("gif", vec!["png".to_string(), "webp".to_string()]);

        assert!(matches!(
            err,
            GenerationError::UnsupportedFormatError { .. }
        ));
    }

    // === Classification tests ===

    #[test]
    fn test_is_retryable() {
        // Retryable errors
        assert!(GenerationError::rate_limit("test", None).is_retryable());
        assert!(GenerationError::timeout(Duration::from_secs(1)).is_retryable());
        assert!(GenerationError::network("test").is_retryable());
        assert!(GenerationError::provider("test", Some(500), "test").is_retryable());
        assert!(GenerationError::provider("test", Some(503), "test").is_retryable());
        assert!(GenerationError::provider("test", Some(429), "test").is_retryable());
        assert!(GenerationError::download("test", None).is_retryable());

        // Non-retryable errors
        assert!(!GenerationError::authentication("test", "test").is_retryable());
        assert!(!GenerationError::invalid_parameters("test", None).is_retryable());
        assert!(!GenerationError::content_filtered("test", None).is_retryable());
        assert!(!GenerationError::cancelled().is_retryable());
    }

    #[test]
    fn test_needs_user_action() {
        // Requires user action
        assert!(GenerationError::authentication("test", "test").needs_user_action());
        assert!(GenerationError::quota_exceeded("test", None).needs_user_action());
        assert!(GenerationError::invalid_parameters("test", None).needs_user_action());
        assert!(GenerationError::content_filtered("test", None).needs_user_action());
        assert!(GenerationError::model_not_found("test", "test").needs_user_action());

        // Does not require user action
        assert!(!GenerationError::rate_limit("test", None).needs_user_action());
        assert!(!GenerationError::timeout(Duration::from_secs(1)).needs_user_action());
        assert!(!GenerationError::network("test").needs_user_action());
    }

    #[test]
    fn test_should_fallback() {
        // Should fallback
        assert!(GenerationError::unsupported_feature("test", "feat", "prov").should_fallback());
        assert!(GenerationError::unsupported_generation_type("image", "prov").should_fallback());
        assert!(GenerationError::unsupported_format("gif", vec![]).should_fallback());
        assert!(GenerationError::unsupported_dimension("test", None).should_fallback());
        assert!(GenerationError::model_not_found("model", "prov").should_fallback());

        // Should not fallback
        assert!(!GenerationError::authentication("test", "test").should_fallback());
        assert!(!GenerationError::rate_limit("test", None).should_fallback());
        assert!(!GenerationError::network("test").should_fallback());
    }

    #[test]
    fn test_retry_after() {
        let with_retry = GenerationError::rate_limit("test", Some(Duration::from_secs(60)));
        assert_eq!(with_retry.retry_after(), Some(Duration::from_secs(60)));

        let without_retry = GenerationError::rate_limit("test", None);
        assert_eq!(without_retry.retry_after(), None);

        let other_error = GenerationError::network("test");
        assert_eq!(other_error.retry_after(), None);
    }

    #[test]
    fn test_provider_extraction() {
        assert_eq!(
            GenerationError::authentication("test", "openai").provider_name(),
            Some("openai")
        );
        assert_eq!(
            GenerationError::provider("test", None, "claude").provider_name(),
            Some("claude")
        );
        assert_eq!(
            GenerationError::model_not_found("model", "gemini").provider_name(),
            Some("gemini")
        );

        // No provider
        assert_eq!(GenerationError::network("test").provider_name(), None);
        assert_eq!(
            GenerationError::timeout(Duration::from_secs(1)).provider_name(),
            None
        );
    }

    // === User-friendly message tests ===

    #[test]
    fn test_user_friendly_message_auth() {
        let err = GenerationError::authentication("Invalid key", "openai");
        let msg = err.user_friendly_message();

        assert!(msg.contains("openai"));
        assert!(msg.contains("API key"));
    }

    #[test]
    fn test_user_friendly_message_rate_limit_with_retry() {
        let err = GenerationError::rate_limit("limit reached", Some(Duration::from_secs(60)));
        let msg = err.user_friendly_message();

        assert!(msg.contains("60"));
        assert!(msg.contains("seconds"));
    }

    #[test]
    fn test_user_friendly_message_rate_limit_without_retry() {
        let err = GenerationError::rate_limit("limit reached", None);
        let msg = err.user_friendly_message();

        assert!(msg.contains("wait"));
    }

    #[test]
    fn test_user_friendly_message_content_filtered() {
        let err = GenerationError::content_filtered("inappropriate", Some("violence".to_string()));
        let msg = err.user_friendly_message();

        assert!(msg.contains("violence"));
        assert!(msg.contains("modify"));
    }

    #[test]
    fn test_user_friendly_message_unsupported_format() {
        let err =
            GenerationError::unsupported_format("gif", vec!["png".to_string(), "webp".to_string()]);
        let msg = err.user_friendly_message();

        assert!(msg.contains("gif"));
        assert!(msg.contains("png, webp"));
    }

    // === Conversion tests ===

    #[test]
    fn test_from_generation_error_to_aleph_error() {
        let gen_err = GenerationError::authentication("Invalid key", "openai");
        let aleph_err: AlephError = gen_err.into();

        assert!(matches!(aleph_err, AlephError::AuthenticationError { .. }));
    }

    #[test]
    fn test_from_rate_limit_to_aleph_error() {
        let gen_err = GenerationError::rate_limit("Too many requests", None);
        let aleph_err: AlephError = gen_err.into();

        assert!(matches!(aleph_err, AlephError::RateLimitError { .. }));
    }

    #[test]
    fn test_from_timeout_to_aleph_error() {
        let gen_err = GenerationError::timeout(Duration::from_secs(30));
        let aleph_err: AlephError = gen_err.into();

        assert!(matches!(aleph_err, AlephError::Timeout { .. }));
    }

    #[test]
    fn test_from_network_to_aleph_error() {
        let gen_err = GenerationError::network("Connection failed");
        let aleph_err: AlephError = gen_err.into();

        assert!(matches!(aleph_err, AlephError::NetworkError { .. }));
    }

    #[test]
    fn test_from_cancelled_to_aleph_error() {
        let gen_err = GenerationError::cancelled();
        let aleph_err: AlephError = gen_err.into();

        assert!(matches!(aleph_err, AlephError::Cancelled));
    }

    #[test]
    fn test_from_provider_error_to_aleph_error() {
        let gen_err = GenerationError::provider("Server error", Some(500), "openai");
        let aleph_err: AlephError = gen_err.into();

        assert!(matches!(aleph_err, AlephError::ProviderError { .. }));
    }

    #[test]
    fn test_cancelled_error() {
        let err = GenerationError::cancelled();

        assert!(matches!(err, GenerationError::Cancelled));
        assert!(!err.is_retryable());
        assert!(!err.needs_user_action());
        assert!(!err.should_fallback());
        assert_eq!(err.user_friendly_message(), "Generation was cancelled.");
    }

    #[test]
    fn test_internal_error() {
        let err = GenerationError::internal("Unexpected state");

        assert!(matches!(err, GenerationError::InternalError { .. }));
        assert!(!err.is_retryable());
    }

    #[test]
    fn test_job_failed_error() {
        let err = GenerationError::job_failed("Processing failed", Some("job-123".to_string()));

        assert!(matches!(err, GenerationError::JobFailedError { .. }));
        let msg = err.user_friendly_message();
        assert!(msg.contains("job-123"));
    }
}
