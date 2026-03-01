/// Mock AI Provider for testing
///
/// This module provides a mock implementation of `AiProvider` for testing
/// without requiring actual API calls or network connectivity.
use crate::error::{AlephError, Result};
use crate::providers::AiProvider;
use std::future::Future;
use std::pin::Pin;
use std::time::Duration;

/// Error type to simulate in MockProvider
#[derive(Clone, Debug)]
pub enum MockError {
    Network(String),
    Authentication(String),
    RateLimit(String),
    Provider(String),
    Timeout,
    NoProviderAvailable,
    InvalidConfig(String),
}

impl From<MockError> for AlephError {
    fn from(err: MockError) -> Self {
        match err {
            MockError::Network(msg) => AlephError::network(msg),
            MockError::Authentication(msg) => AlephError::authentication("Mock".to_string(), msg),
            MockError::RateLimit(msg) => AlephError::rate_limit(msg),
            MockError::Provider(msg) => AlephError::provider(msg),
            MockError::Timeout => AlephError::Timeout {
                suggestion: Some("Try again in a few moments".to_string()),
            },
            MockError::NoProviderAvailable => AlephError::NoProviderAvailable {
                suggestion: Some("Configure a provider".to_string()),
            },
            MockError::InvalidConfig(msg) => AlephError::invalid_config(msg),
        }
    }
}

/// Mock provider for testing AI functionality
///
/// # Features
///
/// - Returns configurable response
/// - Can simulate delays (for timeout testing)
/// - Can simulate errors (for error handling testing)
/// - Thread-safe and async
///
/// # Example
///
/// ```rust,ignore
/// use alephcore::providers::{MockProvider, MockError};
/// use std::time::Duration;
///
/// # tokio_test::block_on(async {
/// // Simple mock
/// let provider = MockProvider::new("mock response");
/// let response = provider.process("input", None).await.unwrap();
/// assert_eq!(response, "mock response");
///
/// // With delay
/// let provider = MockProvider::new("response")
///     .with_delay(Duration::from_millis(100));
/// let response = provider.process("input", None).await.unwrap();
///
/// // With error
/// let provider = MockProvider::new("ignored")
///     .with_error(MockError::Timeout);
/// let result = provider.process("input", None).await;
/// assert!(result.is_err());
/// # });
/// ```
#[derive(Clone)]
pub struct MockProvider {
    response: String,
    delay: Option<Duration>,
    error: Option<MockError>,
    name: String,
    color: String,
}

impl MockProvider {
    /// Create a new mock provider with predefined response
    ///
    /// # Arguments
    ///
    /// * `response` - The response to return from `process()`
    pub fn new(response: impl Into<String>) -> Self {
        Self {
            response: response.into(),
            delay: None,
            error: None,
            name: "mock".to_string(),
            color: "#000000".to_string(),
        }
    }

    /// Add a delay before returning response (for timeout testing)
    ///
    /// # Arguments
    ///
    /// * `delay` - Duration to sleep before returning
    pub fn with_delay(mut self, delay: Duration) -> Self {
        self.delay = Some(delay);
        self
    }

    /// Configure provider to return an error (for error handling testing)
    ///
    /// # Arguments
    ///
    /// * `error` - The error to return from `process()`
    pub fn with_error(mut self, error: MockError) -> Self {
        self.error = Some(error);
        self
    }

    /// Set custom provider name
    pub fn with_name(mut self, name: impl Into<String>) -> Self {
        self.name = name.into();
        self
    }

    /// Set custom provider color
    pub fn with_color(mut self, color: impl Into<String>) -> Self {
        self.color = color.into();
        self
    }
}

impl AiProvider for MockProvider {
    fn process(
        &self,
        _input: &str,
        _system_prompt: Option<&str>,
    ) -> Pin<Box<dyn Future<Output = Result<String>> + Send + '_>> {
        Box::pin(async move {
            // Simulate delay if configured
            if let Some(delay) = self.delay {
                tokio::time::sleep(delay).await;
            }

            // Return error if configured
            if let Some(error) = &self.error {
                return Err(error.clone().into());
            }

            // Return configured response
            Ok(self.response.clone())
        })
    }

    fn name(&self) -> &str {
        &self.name
    }

    fn color(&self) -> &str {
        &self.color
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_mock_provider_basic() {
        let provider = MockProvider::new("test response");

        let response = provider.process("any input", None).await.unwrap();
        assert_eq!(response, "test response");

        assert_eq!(provider.name(), "mock");
        assert_eq!(provider.color(), "#000000");
    }

    #[tokio::test]
    async fn test_mock_provider_with_system_prompt() {
        let provider = MockProvider::new("response");

        let response = provider
            .process("input", Some("system prompt"))
            .await
            .unwrap();
        assert_eq!(response, "response");
    }

    #[tokio::test]
    async fn test_mock_provider_with_delay() {
        let provider = MockProvider::new("delayed").with_delay(Duration::from_millis(50));

        let start = std::time::Instant::now();
        let response = provider.process("input", None).await.unwrap();
        let elapsed = start.elapsed();

        assert_eq!(response, "delayed");
        assert!(elapsed >= Duration::from_millis(50));
    }

    #[tokio::test]
    async fn test_mock_provider_with_error() {
        let provider = MockProvider::new("ignored")
            .with_error(MockError::Authentication("invalid key".to_string()));

        let result = provider.process("input", None).await;
        assert!(result.is_err());

        if let Err(AlephError::AuthenticationError { message, .. }) = result {
            assert_eq!(message, "invalid key");
        } else {
            panic!("Expected AuthenticationError");
        }
    }

    #[tokio::test]
    async fn test_mock_provider_timeout_error() {
        let provider = MockProvider::new("ignored").with_error(MockError::Timeout);

        let result = provider.process("input", None).await;
        assert!(matches!(result, Err(AlephError::Timeout { .. })));
    }

    #[tokio::test]
    async fn test_mock_provider_custom_name_color() {
        let provider = MockProvider::new("response")
            .with_name("custom")
            .with_color("#ffffff");

        assert_eq!(provider.name(), "custom");
        assert_eq!(provider.color(), "#ffffff");

        let response = provider.process("input", None).await.unwrap();
        assert_eq!(response, "response");
    }

    #[test]
    fn test_mock_provider_is_clone() {
        let provider = MockProvider::new("test");
        let cloned = provider.clone();

        assert_eq!(cloned.name(), provider.name());
        assert_eq!(cloned.color(), provider.color());
    }

    #[tokio::test]
    async fn test_mock_provider_in_arc() {
        use crate::sync_primitives::Arc;

        let provider: Arc<dyn AiProvider> = Arc::new(MockProvider::new("test"));

        let response = provider.process("input", None).await.unwrap();
        assert_eq!(response, "test");
    }
}
