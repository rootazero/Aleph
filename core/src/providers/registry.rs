/// Provider Registry for managing AI providers
///
/// This module provides a registry to store and retrieve AI providers by name.
/// It ensures providers are uniquely registered and provides convenient lookup methods.
use crate::error::{AetherError, Result};
use crate::providers::AiProvider;
use std::collections::HashMap;
use std::sync::Arc;

/// Registry for managing AI providers
///
/// # Example
///
/// ```rust
/// use aethecore::providers::{ProviderRegistry, MockProvider};
/// use std::sync::Arc;
///
/// let mut registry = ProviderRegistry::new();
///
/// // Register a provider
/// let provider = Arc::new(MockProvider::new("test"));
/// registry.register("openai".to_string(), provider).unwrap();
///
/// // Retrieve a provider
/// let provider = registry.get("openai").unwrap();
/// assert_eq!(provider.name(), "mock");
///
/// // Check if provider exists
/// assert!(registry.contains("openai"));
///
/// // List all providers
/// let names = registry.names();
/// assert_eq!(names, vec!["openai"]);
/// ```
pub struct ProviderRegistry {
    providers: HashMap<String, Arc<dyn AiProvider>>,
}

impl ProviderRegistry {
    /// Create a new empty provider registry
    ///
    /// # Example
    ///
    /// ```rust
    /// use aethecore::providers::ProviderRegistry;
    ///
    /// let registry = ProviderRegistry::new();
    /// assert_eq!(registry.names().len(), 0);
    /// ```
    pub fn new() -> Self {
        Self {
            providers: HashMap::new(),
        }
    }

    /// Register a provider with a unique name
    ///
    /// # Arguments
    ///
    /// * `name` - Unique identifier for the provider (e.g., "openai", "claude")
    /// * `provider` - Arc-wrapped provider implementation
    ///
    /// # Returns
    ///
    /// * `Ok(())` - Provider registered successfully
    /// * `Err(AetherError::InvalidConfig)` - Provider name already exists
    ///
    /// # Example
    ///
    /// ```rust
    /// use aethecore::providers::{ProviderRegistry, MockProvider};
    /// use std::sync::Arc;
    ///
    /// let mut registry = ProviderRegistry::new();
    /// let provider = Arc::new(MockProvider::new("response"));
    ///
    /// // First registration succeeds
    /// registry.register("openai".to_string(), provider.clone()).unwrap();
    ///
    /// // Duplicate registration fails
    /// let result = registry.register("openai".to_string(), provider);
    /// assert!(result.is_err());
    /// ```
    pub fn register(&mut self, name: String, provider: Arc<dyn AiProvider>) -> Result<()> {
        if self.providers.contains_key(&name) {
            return Err(AetherError::invalid_config(format!(
                "Provider '{}' is already registered",
                name
            )));
        }
        self.providers.insert(name, provider);
        Ok(())
    }

    /// Get a provider by name
    ///
    /// # Arguments
    ///
    /// * `name` - Provider name to look up
    ///
    /// # Returns
    ///
    /// * `Some(Arc<dyn AiProvider>)` - Provider found
    /// * `None` - Provider not found
    ///
    /// # Example
    ///
    /// ```rust
    /// use aethecore::providers::{ProviderRegistry, MockProvider};
    /// use std::sync::Arc;
    ///
    /// let mut registry = ProviderRegistry::new();
    /// registry.register(
    ///     "openai".to_string(),
    ///     Arc::new(MockProvider::new("test"))
    /// ).unwrap();
    ///
    /// let provider = registry.get("openai").unwrap();
    /// assert_eq!(provider.name(), "mock");
    ///
    /// assert!(registry.get("nonexistent").is_none());
    /// ```
    pub fn get(&self, name: &str) -> Option<Arc<dyn AiProvider>> {
        self.providers.get(name).cloned()
    }

    /// Get all registered provider names in sorted order
    ///
    /// # Returns
    ///
    /// Vector of provider names sorted alphabetically
    ///
    /// # Example
    ///
    /// ```rust
    /// use aethecore::providers::{ProviderRegistry, MockProvider};
    /// use std::sync::Arc;
    ///
    /// let mut registry = ProviderRegistry::new();
    /// registry.register("claude".to_string(), Arc::new(MockProvider::new("test"))).unwrap();
    /// registry.register("openai".to_string(), Arc::new(MockProvider::new("test"))).unwrap();
    ///
    /// let names = registry.names();
    /// assert_eq!(names, vec!["claude", "openai"]);
    /// ```
    pub fn names(&self) -> Vec<String> {
        let mut names: Vec<_> = self.providers.keys().cloned().collect();
        names.sort();
        names
    }

    /// Check if a provider is registered
    ///
    /// # Arguments
    ///
    /// * `name` - Provider name to check
    ///
    /// # Returns
    ///
    /// `true` if provider exists, `false` otherwise
    ///
    /// # Example
    ///
    /// ```rust
    /// use aethecore::providers::{ProviderRegistry, MockProvider};
    /// use std::sync::Arc;
    ///
    /// let mut registry = ProviderRegistry::new();
    /// registry.register("openai".to_string(), Arc::new(MockProvider::new("test"))).unwrap();
    ///
    /// assert!(registry.contains("openai"));
    /// assert!(!registry.contains("claude"));
    /// ```
    pub fn contains(&self, name: &str) -> bool {
        self.providers.contains_key(name)
    }

    /// Get the number of registered providers
    ///
    /// # Example
    ///
    /// ```rust
    /// use aethecore::providers::{ProviderRegistry, MockProvider};
    /// use std::sync::Arc;
    ///
    /// let mut registry = ProviderRegistry::new();
    /// assert_eq!(registry.len(), 0);
    ///
    /// registry.register("openai".to_string(), Arc::new(MockProvider::new("test"))).unwrap();
    /// assert_eq!(registry.len(), 1);
    /// ```
    pub fn len(&self) -> usize {
        self.providers.len()
    }

    /// Check if the registry is empty
    ///
    /// # Example
    ///
    /// ```rust
    /// use aethecore::providers::ProviderRegistry;
    ///
    /// let registry = ProviderRegistry::new();
    /// assert!(registry.is_empty());
    /// ```
    pub fn is_empty(&self) -> bool {
        self.providers.is_empty()
    }
}

impl Default for ProviderRegistry {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::providers::MockProvider;

    #[test]
    fn test_registry_new() {
        let registry = ProviderRegistry::new();
        assert_eq!(registry.len(), 0);
        assert!(registry.is_empty());
    }

    #[test]
    fn test_registry_register() {
        let mut registry = ProviderRegistry::new();
        let provider = Arc::new(MockProvider::new("test"));

        let result = registry.register("openai".to_string(), provider);
        assert!(result.is_ok());
        assert_eq!(registry.len(), 1);
    }

    #[test]
    fn test_registry_register_duplicate() {
        let mut registry = ProviderRegistry::new();
        let provider1 = Arc::new(MockProvider::new("test1"));
        let provider2 = Arc::new(MockProvider::new("test2"));

        registry.register("openai".to_string(), provider1).unwrap();

        let result = registry.register("openai".to_string(), provider2);
        assert!(result.is_err());

        if let Err(AetherError::InvalidConfig { message, .. }) = result {
            assert!(message.contains("already registered"));
        } else {
            panic!("Expected InvalidConfig error");
        }
    }

    #[test]
    fn test_registry_get() {
        let mut registry = ProviderRegistry::new();
        let provider = Arc::new(MockProvider::new("test response"));

        registry.register("openai".to_string(), provider).unwrap();

        let retrieved = registry.get("openai").unwrap();
        assert_eq!(retrieved.name(), "mock");
    }

    #[test]
    fn test_registry_get_nonexistent() {
        let registry = ProviderRegistry::new();
        assert!(registry.get("nonexistent").is_none());
    }

    #[test]
    fn test_registry_contains() {
        let mut registry = ProviderRegistry::new();
        let provider = Arc::new(MockProvider::new("test"));

        registry.register("openai".to_string(), provider).unwrap();

        assert!(registry.contains("openai"));
        assert!(!registry.contains("claude"));
    }

    #[test]
    fn test_registry_names() {
        let mut registry = ProviderRegistry::new();

        registry
            .register("claude".to_string(), Arc::new(MockProvider::new("test")))
            .unwrap();
        registry
            .register("openai".to_string(), Arc::new(MockProvider::new("test")))
            .unwrap();
        registry
            .register("ollama".to_string(), Arc::new(MockProvider::new("test")))
            .unwrap();

        let names = registry.names();
        assert_eq!(names, vec!["claude", "ollama", "openai"]);
    }

    #[test]
    fn test_registry_len() {
        let mut registry = ProviderRegistry::new();
        assert_eq!(registry.len(), 0);

        registry
            .register("openai".to_string(), Arc::new(MockProvider::new("test")))
            .unwrap();
        assert_eq!(registry.len(), 1);

        registry
            .register("claude".to_string(), Arc::new(MockProvider::new("test")))
            .unwrap();
        assert_eq!(registry.len(), 2);
    }

    #[test]
    fn test_registry_is_empty() {
        let mut registry = ProviderRegistry::new();
        assert!(registry.is_empty());

        registry
            .register("openai".to_string(), Arc::new(MockProvider::new("test")))
            .unwrap();
        assert!(!registry.is_empty());
    }

    #[test]
    fn test_registry_default() {
        let registry = ProviderRegistry::default();
        assert!(registry.is_empty());
    }

    #[tokio::test]
    async fn test_registry_provider_usage() {
        let mut registry = ProviderRegistry::new();
        let provider = Arc::new(MockProvider::new("AI response"));

        registry.register("test".to_string(), provider).unwrap();

        let provider = registry.get("test").unwrap();
        let response = provider.process("input", None).await.unwrap();
        assert_eq!(response, "AI response");
    }

    #[test]
    fn test_registry_multiple_providers() {
        let mut registry = ProviderRegistry::new();

        let openai = Arc::new(MockProvider::new("openai response").with_name("openai"));
        let claude = Arc::new(MockProvider::new("claude response").with_name("claude"));
        let ollama = Arc::new(MockProvider::new("ollama response").with_name("ollama"));

        registry.register("openai".to_string(), openai).unwrap();
        registry.register("claude".to_string(), claude).unwrap();
        registry.register("ollama".to_string(), ollama).unwrap();

        assert_eq!(registry.len(), 3);
        assert!(registry.contains("openai"));
        assert!(registry.contains("claude"));
        assert!(registry.contains("ollama"));

        let openai_provider = registry.get("openai").unwrap();
        assert_eq!(openai_provider.name(), "openai");
    }
}
