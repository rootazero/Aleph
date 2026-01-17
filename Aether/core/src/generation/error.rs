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
use crate::error::AetherError;
use std::time::Duration;
use thiserror::Error;

/// Errors that can occur during media generation operations
///
/// # Example
///
/// ```rust
/// use aethecore::generation::GenerationError;
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
    AuthenticationError {
        message: String,
        provider: String,
    },

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
    TimeoutError {
        duration: Duration,
    },

    /// Network connectivity error
    #[error("Network error: {message}")]
    NetworkError {
        message: String,
    },

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
    InternalError {
        message: String,
    },

    /// Model not found or not available
    #[error("Model not found: {model}")]
    ModelNotFoundError {
        model: String,
        provider: String,
    },

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
    SerializationError {
        message: String,
    },
}

impl GenerationError {
    // === Factory methods ===

    /// Create an authentication error
    pub fn authentication<S: Into<String>, P: Into<String>>(message: S, provider: P) -> Self {
        GenerationError::AuthenticationError {
            message: message.into(),
            provider: provider.into(),
        }
    }

    /// Create a rate limit error
    pub fn rate_limit<S: Into<String>>(message: S, retry_after: Option<Duration>) -> Self {
        GenerationError::RateLimitError {
            message: message.into(),
            retry_after,
        }
    }

    /// Create a quota exceeded error
    pub fn quota_exceeded<S: Into<String>>(message: S, resets_at: Option<String>) -> Self {
        GenerationError::QuotaExceededError {
            message: message.into(),
            resets_at,
        }
    }

    /// Create a timeout error
    pub fn timeout(duration: Duration) -> Self {
        GenerationError::TimeoutError { duration }
    }

    /// Create a network error
    pub fn network<S: Into<String>>(message: S) -> Self {
        GenerationError::NetworkError {
            message: message.into(),
        }
    }

    /// Create an invalid parameters error
    pub fn invalid_parameters<S: Into<String>>(message: S, parameter: Option<String>) -> Self {
        GenerationError::InvalidParametersError {
            message: message.into(),
            parameter,
        }
    }

    /// Create a content filtered error
    pub fn content_filtered<S: Into<String>>(message: S, category: Option<String>) -> Self {
        GenerationError::ContentFilteredError {
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
        GenerationError::UnsupportedFeatureError {
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
        GenerationError::ProviderError {
            message: message.into(),
            status_code,
            provider: provider.into(),
        }
    }

    /// Create a cancelled error
    pub fn cancelled() -> Self {
        GenerationError::Cancelled
    }

    /// Create an internal error
    pub fn internal<S: Into<String>>(message: S) -> Self {
        GenerationError::InternalError {
            message: message.into(),
        }
    }

    /// Create a model not found error
    pub fn model_not_found<M: Into<String>, P: Into<String>>(model: M, provider: P) -> Self {
        GenerationError::ModelNotFoundError {
            model: model.into(),
            provider: provider.into(),
        }
    }

    /// Create an unsupported generation type error
    pub fn unsupported_generation_type<G: Into<String>, P: Into<String>>(
        generation_type: G,
        provider: P,
    ) -> Self {
        GenerationError::UnsupportedGenerationTypeError {
            generation_type: generation_type.into(),
            provider: provider.into(),
        }
    }

    /// Create an unsupported format error
    pub fn unsupported_format<F: Into<String>>(format: F, supported: Vec<String>) -> Self {
        GenerationError::UnsupportedFormatError {
            format: format.into(),
            supported,
        }
    }

    /// Create an unsupported dimension error
    pub fn unsupported_dimension<S: Into<String>>(message: S, suggested: Option<String>) -> Self {
        GenerationError::UnsupportedDimensionError {
            message: message.into(),
            suggested,
        }
    }

    /// Create a job failed error
    pub fn job_failed<S: Into<String>>(message: S, job_id: Option<String>) -> Self {
        GenerationError::JobFailedError {
            message: message.into(),
            job_id,
        }
    }

    /// Create a download error
    pub fn download<S: Into<String>>(message: S, url: Option<String>) -> Self {
        GenerationError::DownloadError {
            message: message.into(),
            url,
        }
    }

    /// Create a serialization error
    pub fn serialization<S: Into<String>>(message: S) -> Self {
        GenerationError::SerializationError {
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
    /// use aethecore::generation::GenerationError;
    /// use std::time::Duration;
    ///
    /// let rate_limit = GenerationError::rate_limit("Too many requests", None);
    /// assert!(rate_limit.is_retryable());
    ///
    /// let auth_error = GenerationError::authentication("Invalid key", "openai");
    /// assert!(!auth_error.is_retryable());
    /// ```
    pub fn is_retryable(&self) -> bool {
        matches!(
            self,
            GenerationError::RateLimitError { .. }
                | GenerationError::TimeoutError { .. }
                | GenerationError::NetworkError { .. }
                | GenerationError::ProviderError { status_code: Some(500..=599), .. }
                | GenerationError::ProviderError { status_code: Some(429), .. }
                | GenerationError::DownloadError { .. }
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
    /// use aethecore::generation::GenerationError;
    ///
    /// let auth_error = GenerationError::authentication("Invalid key", "openai");
    /// assert!(auth_error.needs_user_action());
    ///
    /// let timeout = GenerationError::timeout(std::time::Duration::from_secs(30));
    /// assert!(!timeout.needs_user_action());
    /// ```
    pub fn needs_user_action(&self) -> bool {
        matches!(
            self,
            GenerationError::AuthenticationError { .. }
                | GenerationError::QuotaExceededError { .. }
                | GenerationError::InvalidParametersError { .. }
                | GenerationError::ContentFilteredError { .. }
                | GenerationError::ModelNotFoundError { .. }
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
    /// use aethecore::generation::GenerationError;
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
    pub fn should_fallback(&self) -> bool {
        matches!(
            self,
            GenerationError::UnsupportedFeatureError { .. }
                | GenerationError::UnsupportedGenerationTypeError { .. }
                | GenerationError::UnsupportedFormatError { .. }
                | GenerationError::UnsupportedDimensionError { .. }
                | GenerationError::ModelNotFoundError { .. }
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
    /// use aethecore::generation::GenerationError;
    /// use std::time::Duration;
    ///
    /// let error = GenerationError::rate_limit(
    ///     "Too many requests",
    ///     Some(Duration::from_secs(60))
    /// );
    ///
    /// assert_eq!(error.retry_after(), Some(Duration::from_secs(60)));
    /// ```
    pub fn retry_after(&self) -> Option<Duration> {
        match self {
            GenerationError::RateLimitError { retry_after, .. } => *retry_after,
            _ => None,
        }
    }

    /// Get the provider name if available
    pub fn provider_name(&self) -> Option<&str> {
        match self {
            GenerationError::AuthenticationError { provider, .. } => Some(provider),
            GenerationError::UnsupportedFeatureError { provider, .. } => Some(provider),
            GenerationError::ProviderError { provider, .. } => Some(provider),
            GenerationError::ModelNotFoundError { provider, .. } => Some(provider),
            GenerationError::UnsupportedGenerationTypeError { provider, .. } => Some(provider),
            _ => None,
        }
    }

    /// Get a user-friendly error message
    ///
    /// Returns a message suitable for display in the UI,
    /// with actionable suggestions where applicable.
    pub fn user_friendly_message(&self) -> String {
        match self {
            GenerationError::AuthenticationError { provider, .. } => {
                format!(
                    "Authentication failed for {}. Please check your API key in settings.",
                    provider
                )
            }
            GenerationError::RateLimitError { retry_after, .. } => {
                if let Some(duration) = retry_after {
                    format!(
                        "Rate limit exceeded. Please wait {} seconds before trying again.",
                        duration.as_secs()
                    )
                } else {
                    "Rate limit exceeded. Please wait a moment before trying again.".to_string()
                }
            }
            GenerationError::QuotaExceededError { resets_at, .. } => {
                if let Some(reset) = resets_at {
                    format!(
                        "Usage quota exceeded. Your quota resets at {}. Consider upgrading your plan.",
                        reset
                    )
                } else {
                    "Usage quota exceeded. Consider upgrading your plan or waiting for quota reset."
                        .to_string()
                }
            }
            GenerationError::TimeoutError { duration } => {
                format!(
                    "Generation timed out after {} seconds. Try a simpler prompt or smaller output.",
                    duration.as_secs()
                )
            }
            GenerationError::NetworkError { .. } => {
                "Network error. Please check your internet connection and try again.".to_string()
            }
            GenerationError::InvalidParametersError { parameter, message } => {
                if let Some(param) = parameter {
                    format!("Invalid parameter '{}': {}. Please adjust your settings.", param, message)
                } else {
                    format!("Invalid parameters: {}. Please check your request.", message)
                }
            }
            GenerationError::ContentFilteredError { category, .. } => {
                if let Some(cat) = category {
                    format!(
                        "Content was filtered for '{}' category. Please modify your prompt.",
                        cat
                    )
                } else {
                    "Content was filtered by safety systems. Please modify your prompt.".to_string()
                }
            }
            GenerationError::UnsupportedFeatureError { feature, provider, .. } => {
                format!(
                    "The feature '{}' is not supported by {}. Try a different provider.",
                    feature, provider
                )
            }
            GenerationError::ProviderError { message, status_code, provider } => {
                if let Some(code) = status_code {
                    format!("{} returned error {}: {}", provider, code, message)
                } else {
                    format!("{} error: {}", provider, message)
                }
            }
            GenerationError::Cancelled => {
                "Generation was cancelled.".to_string()
            }
            GenerationError::InternalError { message } => {
                format!("Internal error: {}. Please try again.", message)
            }
            GenerationError::ModelNotFoundError { model, provider } => {
                format!(
                    "Model '{}' not found on {}. Check the model name or try a different model.",
                    model, provider
                )
            }
            GenerationError::UnsupportedGenerationTypeError { generation_type, provider } => {
                format!(
                    "{} does not support {} generation. Try a different provider.",
                    provider, generation_type
                )
            }
            GenerationError::UnsupportedFormatError { format, supported } => {
                format!(
                    "Output format '{}' is not supported. Supported formats: {}",
                    format,
                    supported.join(", ")
                )
            }
            GenerationError::UnsupportedDimensionError { message, suggested } => {
                if let Some(sug) = suggested {
                    format!("{}. Suggested: {}", message, sug)
                } else {
                    message.clone()
                }
            }
            GenerationError::JobFailedError { message, job_id } => {
                if let Some(id) = job_id {
                    format!("Generation job {} failed: {}", id, message)
                } else {
                    format!("Generation job failed: {}", message)
                }
            }
            GenerationError::DownloadError { message, url } => {
                if let Some(u) = url {
                    format!("Failed to download from {}: {}", u, message)
                } else {
                    format!("Download failed: {}", message)
                }
            }
            GenerationError::SerializationError { message } => {
                format!("Data processing error: {}. Please try again.", message)
            }
        }
    }
}

/// Convert GenerationError to AetherError for integration with core error handling
impl From<GenerationError> for AetherError {
    fn from(err: GenerationError) -> Self {
        match err {
            GenerationError::AuthenticationError { message, provider } => {
                AetherError::authentication(provider, message)
            }
            GenerationError::RateLimitError { message, .. } => {
                AetherError::rate_limit(message)
            }
            GenerationError::QuotaExceededError { message, .. } => {
                AetherError::rate_limit(format!("Quota exceeded: {}", message))
            }
            GenerationError::TimeoutError { duration } => {
                AetherError::Timeout {
                    suggestion: Some(format!(
                        "Generation timed out after {} seconds. Try a simpler request.",
                        duration.as_secs()
                    )),
                }
            }
            GenerationError::NetworkError { message } => {
                AetherError::network(message)
            }
            GenerationError::InvalidParametersError { message, .. } => {
                AetherError::invalid_config(message)
            }
            GenerationError::ContentFilteredError { message, .. } => {
                AetherError::provider(format!("Content filtered: {}", message))
            }
            GenerationError::UnsupportedFeatureError { message, .. } => {
                AetherError::provider(message)
            }
            GenerationError::ProviderError { message, .. } => {
                AetherError::provider(message)
            }
            GenerationError::Cancelled => {
                AetherError::cancelled()
            }
            GenerationError::InternalError { message } => {
                AetherError::other(message)
            }
            GenerationError::ModelNotFoundError { model, provider } => {
                AetherError::invalid_config(format!(
                    "Model '{}' not found on {}",
                    model, provider
                ))
            }
            GenerationError::UnsupportedGenerationTypeError { generation_type, provider } => {
                AetherError::provider(format!(
                    "{} does not support {} generation",
                    provider, generation_type
                ))
            }
            GenerationError::UnsupportedFormatError { format, .. } => {
                AetherError::invalid_config(format!("Unsupported format: {}", format))
            }
            GenerationError::UnsupportedDimensionError { message, .. } => {
                AetherError::invalid_config(message)
            }
            GenerationError::JobFailedError { message, .. } => {
                AetherError::provider(message)
            }
            GenerationError::DownloadError { message, .. } => {
                AetherError::network(message)
            }
            GenerationError::SerializationError { message } => {
                AetherError::IoError(message)
            }
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

        assert!(matches!(err, GenerationError::UnsupportedFeatureError { .. }));
        assert_eq!(err.provider_name(), Some("dall-e-3"));
    }

    #[test]
    fn test_provider_error() {
        let err = GenerationError::provider(
            "Internal server error",
            Some(500),
            "openai",
        );

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
        let err = GenerationError::unsupported_format(
            "gif",
            vec!["png".to_string(), "webp".to_string()],
        );

        assert!(matches!(err, GenerationError::UnsupportedFormatError { .. }));
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
        assert_eq!(GenerationError::timeout(Duration::from_secs(1)).provider_name(), None);
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
        let err = GenerationError::unsupported_format(
            "gif",
            vec!["png".to_string(), "webp".to_string()],
        );
        let msg = err.user_friendly_message();

        assert!(msg.contains("gif"));
        assert!(msg.contains("png, webp"));
    }

    // === Conversion tests ===

    #[test]
    fn test_from_generation_error_to_aether_error() {
        let gen_err = GenerationError::authentication("Invalid key", "openai");
        let aether_err: AetherError = gen_err.into();

        assert!(matches!(aether_err, AetherError::AuthenticationError { .. }));
    }

    #[test]
    fn test_from_rate_limit_to_aether_error() {
        let gen_err = GenerationError::rate_limit("Too many requests", None);
        let aether_err: AetherError = gen_err.into();

        assert!(matches!(aether_err, AetherError::RateLimitError { .. }));
    }

    #[test]
    fn test_from_timeout_to_aether_error() {
        let gen_err = GenerationError::timeout(Duration::from_secs(30));
        let aether_err: AetherError = gen_err.into();

        assert!(matches!(aether_err, AetherError::Timeout { .. }));
    }

    #[test]
    fn test_from_network_to_aether_error() {
        let gen_err = GenerationError::network("Connection failed");
        let aether_err: AetherError = gen_err.into();

        assert!(matches!(aether_err, AetherError::NetworkError { .. }));
    }

    #[test]
    fn test_from_cancelled_to_aether_error() {
        let gen_err = GenerationError::cancelled();
        let aether_err: AetherError = gen_err.into();

        assert!(matches!(aether_err, AetherError::Cancelled));
    }

    #[test]
    fn test_from_provider_error_to_aether_error() {
        let gen_err = GenerationError::provider("Server error", Some(500), "openai");
        let aether_err: AetherError = gen_err.into();

        assert!(matches!(aether_err, AetherError::ProviderError { .. }));
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
