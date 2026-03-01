/// Registry for managing generation providers
///
/// This module provides a registry to store and retrieve generation providers by name.
/// It supports filtering providers by generation type and provides convenient lookup methods.
///
/// # Example
///
/// ```rust,ignore
/// use alephcore::generation::{
///     GenerationProviderRegistry, GenerationType, MockGenerationProvider,
/// };
/// use std::sync::Arc;
///
/// let mut registry = GenerationProviderRegistry::new();
///
/// // Register a provider
/// let provider = Arc::new(MockGenerationProvider::new("dalle"));
/// registry.register("dalle".to_string(), provider).unwrap();
///
/// // Retrieve by name
/// let provider = registry.get("dalle").unwrap();
///
/// // Get providers supporting a specific type
/// let image_providers = registry.providers_for_type(GenerationType::Image);
/// ```
use crate::generation::error::{GenerationError, GenerationResult};
use crate::generation::types::GenerationType;
use crate::generation::GenerationProvider;
use std::collections::HashMap;
use crate::sync_primitives::Arc;

/// Registry for managing generation providers
///
/// Stores generation providers indexed by name and provides methods for:
/// - Registering and removing providers
/// - Looking up providers by name
/// - Filtering providers by supported generation types
///
/// # Thread Safety
///
/// The registry itself is not thread-safe. For concurrent access,
/// wrap it in an `Arc<RwLock<GenerationProviderRegistry>>`.
pub struct GenerationProviderRegistry {
    providers: HashMap<String, Arc<dyn GenerationProvider>>,
}

impl GenerationProviderRegistry {
    /// Create a new empty provider registry
    ///
    /// # Example
    ///
    /// ```rust
    /// use alephcore::generation::GenerationProviderRegistry;
    ///
    /// let registry = GenerationProviderRegistry::new();
    /// assert!(registry.is_empty());
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
    /// * `name` - Unique identifier for the provider
    /// * `provider` - Arc-wrapped provider implementation
    ///
    /// # Returns
    ///
    /// * `Ok(())` - Provider registered successfully
    /// * `Err(GenerationError)` - Provider name already exists
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// use alephcore::generation::{GenerationProviderRegistry, MockGenerationProvider};
    /// use std::sync::Arc;
    ///
    /// let mut registry = GenerationProviderRegistry::new();
    /// let provider = Arc::new(MockGenerationProvider::new("dalle"));
    ///
    /// // First registration succeeds
    /// registry.register("dalle".to_string(), provider.clone()).unwrap();
    ///
    /// // Duplicate registration fails
    /// let result = registry.register("dalle".to_string(), provider);
    /// assert!(result.is_err());
    /// ```
    pub fn register(
        &mut self,
        name: String,
        provider: Arc<dyn GenerationProvider>,
    ) -> GenerationResult<()> {
        if self.providers.contains_key(&name) {
            return Err(GenerationError::internal(format!(
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
    /// * `Some(Arc<dyn GenerationProvider>)` - Provider found
    /// * `None` - Provider not found
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// use alephcore::generation::{GenerationProviderRegistry, MockGenerationProvider};
    /// use std::sync::Arc;
    ///
    /// let mut registry = GenerationProviderRegistry::new();
    /// registry.register(
    ///     "dalle".to_string(),
    ///     Arc::new(MockGenerationProvider::new("dalle"))
    /// ).unwrap();
    ///
    /// let provider = registry.get("dalle");
    /// assert!(provider.is_some());
    ///
    /// let missing = registry.get("nonexistent");
    /// assert!(missing.is_none());
    /// ```
    pub fn get(&self, name: &str) -> Option<Arc<dyn GenerationProvider>> {
        self.providers.get(name).cloned()
    }

    /// Get a provider by name or return an error
    ///
    /// This is a convenience method that returns an error instead of `None`.
    ///
    /// # Arguments
    ///
    /// * `name` - Provider name to look up
    ///
    /// # Returns
    ///
    /// * `Ok(Arc<dyn GenerationProvider>)` - Provider found
    /// * `Err(GenerationError)` - Provider not found
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// use alephcore::generation::{GenerationProviderRegistry, MockGenerationProvider};
    /// use std::sync::Arc;
    ///
    /// let mut registry = GenerationProviderRegistry::new();
    /// registry.register(
    ///     "dalle".to_string(),
    ///     Arc::new(MockGenerationProvider::new("dalle"))
    /// ).unwrap();
    ///
    /// let provider = registry.get_or_err("dalle").unwrap();
    /// assert_eq!(provider.name(), "dalle");
    ///
    /// let result = registry.get_or_err("nonexistent");
    /// assert!(result.is_err());
    /// ```
    pub fn get_or_err(&self, name: &str) -> GenerationResult<Arc<dyn GenerationProvider>> {
        self.get(name)
            .ok_or_else(|| GenerationError::internal(format!("Provider '{}' not found", name)))
    }

    /// Get all registered provider names in sorted order
    ///
    /// # Returns
    ///
    /// Vector of provider names sorted alphabetically
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// use alephcore::generation::{GenerationProviderRegistry, MockGenerationProvider};
    /// use std::sync::Arc;
    ///
    /// let mut registry = GenerationProviderRegistry::new();
    /// registry.register("dalle".to_string(), Arc::new(MockGenerationProvider::new("dalle"))).unwrap();
    /// registry.register("midjourney".to_string(), Arc::new(MockGenerationProvider::new("midjourney"))).unwrap();
    ///
    /// let names = registry.names();
    /// assert_eq!(names, vec!["dalle", "midjourney"]);
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
    pub fn contains(&self, name: &str) -> bool {
        self.providers.contains_key(name)
    }

    /// Get the number of registered providers
    pub fn len(&self) -> usize {
        self.providers.len()
    }

    /// Check if the registry is empty
    pub fn is_empty(&self) -> bool {
        self.providers.is_empty()
    }

    /// Get all providers that support a specific generation type
    ///
    /// # Arguments
    ///
    /// * `gen_type` - The generation type to filter by
    ///
    /// # Returns
    ///
    /// Vector of Arc-wrapped providers that support the given type
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// use alephcore::generation::{
    ///     GenerationProviderRegistry, GenerationType, MockGenerationProvider,
    /// };
    /// use std::sync::Arc;
    ///
    /// let mut registry = GenerationProviderRegistry::new();
    /// registry.register("dalle".to_string(), Arc::new(MockGenerationProvider::new("dalle"))).unwrap();
    ///
    /// let image_providers = registry.providers_for_type(GenerationType::Image);
    /// assert_eq!(image_providers.len(), 1);
    /// ```
    pub fn providers_for_type(&self, gen_type: GenerationType) -> Vec<Arc<dyn GenerationProvider>> {
        self.providers
            .values()
            .filter(|p| p.supported_types().contains(&gen_type))
            .cloned()
            .collect()
    }

    /// Get names of all providers that support a specific generation type
    ///
    /// # Arguments
    ///
    /// * `gen_type` - The generation type to filter by
    ///
    /// # Returns
    ///
    /// Vector of provider names that support the given type (sorted)
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// use alephcore::generation::{
    ///     GenerationProviderRegistry, GenerationType, MockGenerationProvider,
    /// };
    /// use std::sync::Arc;
    ///
    /// let mut registry = GenerationProviderRegistry::new();
    /// registry.register("dalle".to_string(), Arc::new(MockGenerationProvider::new("dalle"))).unwrap();
    ///
    /// let names = registry.names_for_type(GenerationType::Image);
    /// assert_eq!(names, vec!["dalle"]);
    /// ```
    pub fn names_for_type(&self, gen_type: GenerationType) -> Vec<String> {
        let mut names: Vec<_> = self
            .providers
            .iter()
            .filter(|(_, p)| p.supported_types().contains(&gen_type))
            .map(|(name, _)| name.clone())
            .collect();
        names.sort();
        names
    }

    /// Remove a provider from the registry
    ///
    /// # Arguments
    ///
    /// * `name` - Provider name to remove
    ///
    /// # Returns
    ///
    /// * `Some(Arc<dyn GenerationProvider>)` - The removed provider
    /// * `None` - Provider was not found
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// use alephcore::generation::{GenerationProviderRegistry, MockGenerationProvider};
    /// use std::sync::Arc;
    ///
    /// let mut registry = GenerationProviderRegistry::new();
    /// registry.register("dalle".to_string(), Arc::new(MockGenerationProvider::new("dalle"))).unwrap();
    ///
    /// let removed = registry.remove("dalle");
    /// assert!(removed.is_some());
    /// assert!(registry.is_empty());
    ///
    /// let not_found = registry.remove("nonexistent");
    /// assert!(not_found.is_none());
    /// ```
    pub fn remove(&mut self, name: &str) -> Option<Arc<dyn GenerationProvider>> {
        self.providers.remove(name)
    }

    /// Remove all providers from the registry
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// use alephcore::generation::{GenerationProviderRegistry, MockGenerationProvider};
    /// use std::sync::Arc;
    ///
    /// let mut registry = GenerationProviderRegistry::new();
    /// registry.register("dalle".to_string(), Arc::new(MockGenerationProvider::new("dalle"))).unwrap();
    /// registry.register("midjourney".to_string(), Arc::new(MockGenerationProvider::new("midjourney"))).unwrap();
    ///
    /// registry.clear();
    /// assert!(registry.is_empty());
    /// ```
    pub fn clear(&mut self) {
        self.providers.clear();
    }

    /// Get an iterator over all providers
    ///
    /// # Returns
    ///
    /// Iterator yielding (name, provider) pairs
    pub fn iter(&self) -> impl Iterator<Item = (&String, &Arc<dyn GenerationProvider>)> {
        self.providers.iter()
    }

    /// Get the first provider that supports a generation type
    ///
    /// Useful for simple cases where you just need any provider for a type.
    ///
    /// # Arguments
    ///
    /// * `gen_type` - The generation type to look for
    ///
    /// # Returns
    ///
    /// * `Some((name, provider))` - A provider that supports the type
    /// * `None` - No provider supports this type
    pub fn first_for_type(
        &self,
        gen_type: GenerationType,
    ) -> Option<(String, Arc<dyn GenerationProvider>)> {
        self.providers
            .iter()
            .find(|(_, p)| p.supported_types().contains(&gen_type))
            .map(|(name, p)| (name.clone(), p.clone()))
    }
}

impl Default for GenerationProviderRegistry {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::generation::{GenerationRequest, MockGenerationProvider};

    fn create_test_provider(name: &str) -> Arc<dyn GenerationProvider> {
        Arc::new(MockGenerationProvider::new(name))
    }

    // === Basic operations tests ===

    #[test]
    fn test_registry_new() {
        let registry = GenerationProviderRegistry::new();
        assert_eq!(registry.len(), 0);
        assert!(registry.is_empty());
    }

    #[test]
    fn test_registry_default() {
        let registry = GenerationProviderRegistry::default();
        assert!(registry.is_empty());
    }

    #[test]
    fn test_registry_register() {
        let mut registry = GenerationProviderRegistry::new();
        let provider = create_test_provider("dalle");

        let result = registry.register("dalle".to_string(), provider);
        assert!(result.is_ok());
        assert_eq!(registry.len(), 1);
        assert!(!registry.is_empty());
    }

    #[test]
    fn test_registry_register_duplicate() {
        let mut registry = GenerationProviderRegistry::new();
        let provider1 = create_test_provider("dalle");
        let provider2 = create_test_provider("dalle2");

        registry.register("dalle".to_string(), provider1).unwrap();

        let result = registry.register("dalle".to_string(), provider2);
        assert!(result.is_err());

        if let Err(GenerationError::InternalError { message }) = result {
            assert!(message.contains("already registered"));
        } else {
            panic!("Expected InternalError");
        }
    }

    #[test]
    fn test_registry_get() {
        let mut registry = GenerationProviderRegistry::new();
        let provider = create_test_provider("dalle");

        registry.register("dalle".to_string(), provider).unwrap();

        let retrieved = registry.get("dalle");
        assert!(retrieved.is_some());
        assert_eq!(retrieved.unwrap().name(), "dalle");
    }

    #[test]
    fn test_registry_get_nonexistent() {
        let registry = GenerationProviderRegistry::new();
        assert!(registry.get("nonexistent").is_none());
    }

    #[test]
    fn test_registry_get_or_err() {
        let mut registry = GenerationProviderRegistry::new();
        let provider = create_test_provider("dalle");

        registry.register("dalle".to_string(), provider).unwrap();

        let result = registry.get_or_err("dalle");
        assert!(result.is_ok());
        assert_eq!(result.unwrap().name(), "dalle");
    }

    #[test]
    fn test_registry_get_or_err_not_found() {
        let registry = GenerationProviderRegistry::new();

        let result = registry.get_or_err("nonexistent");
        assert!(result.is_err());

        if let Err(GenerationError::InternalError { message }) = result {
            assert!(message.contains("not found"));
        } else {
            panic!("Expected InternalError");
        }
    }

    #[test]
    fn test_registry_contains() {
        let mut registry = GenerationProviderRegistry::new();
        let provider = create_test_provider("dalle");

        registry.register("dalle".to_string(), provider).unwrap();

        assert!(registry.contains("dalle"));
        assert!(!registry.contains("midjourney"));
    }

    #[test]
    fn test_registry_names() {
        let mut registry = GenerationProviderRegistry::new();

        registry
            .register("dalle".to_string(), create_test_provider("dalle"))
            .unwrap();
        registry
            .register("midjourney".to_string(), create_test_provider("midjourney"))
            .unwrap();
        registry
            .register("stable-diffusion".to_string(), create_test_provider("sd"))
            .unwrap();

        let names = registry.names();
        assert_eq!(names, vec!["dalle", "midjourney", "stable-diffusion"]);
    }

    #[test]
    fn test_registry_len() {
        let mut registry = GenerationProviderRegistry::new();
        assert_eq!(registry.len(), 0);

        registry
            .register("dalle".to_string(), create_test_provider("dalle"))
            .unwrap();
        assert_eq!(registry.len(), 1);

        registry
            .register("midjourney".to_string(), create_test_provider("mj"))
            .unwrap();
        assert_eq!(registry.len(), 2);
    }

    // === Type filtering tests ===

    #[test]
    fn test_registry_providers_for_type() {
        let mut registry = GenerationProviderRegistry::new();

        registry
            .register("dalle".to_string(), create_test_provider("dalle"))
            .unwrap();
        registry
            .register("midjourney".to_string(), create_test_provider("mj"))
            .unwrap();

        let image_providers = registry.providers_for_type(GenerationType::Image);
        assert_eq!(image_providers.len(), 2);
    }

    #[test]
    fn test_registry_names_for_type() {
        let mut registry = GenerationProviderRegistry::new();

        registry
            .register("dalle".to_string(), create_test_provider("dalle"))
            .unwrap();
        registry
            .register("midjourney".to_string(), create_test_provider("mj"))
            .unwrap();

        let names = registry.names_for_type(GenerationType::Image);
        assert_eq!(names, vec!["dalle", "midjourney"]);
    }

    #[test]
    fn test_registry_first_for_type() {
        let mut registry = GenerationProviderRegistry::new();

        registry
            .register("dalle".to_string(), create_test_provider("dalle"))
            .unwrap();

        let result = registry.first_for_type(GenerationType::Image);
        assert!(result.is_some());

        let (name, provider) = result.unwrap();
        assert_eq!(name, "dalle");
        assert_eq!(provider.name(), "dalle");
    }

    #[test]
    fn test_registry_first_for_type_not_found() {
        let registry = GenerationProviderRegistry::new();

        let result = registry.first_for_type(GenerationType::Video);
        assert!(result.is_none());
    }

    // === Modification tests ===

    #[test]
    fn test_registry_remove() {
        let mut registry = GenerationProviderRegistry::new();
        registry
            .register("dalle".to_string(), create_test_provider("dalle"))
            .unwrap();

        let removed = registry.remove("dalle");
        assert!(removed.is_some());
        assert!(registry.is_empty());
        assert!(!registry.contains("dalle"));
    }

    #[test]
    fn test_registry_remove_nonexistent() {
        let mut registry = GenerationProviderRegistry::new();

        let removed = registry.remove("nonexistent");
        assert!(removed.is_none());
    }

    #[test]
    fn test_registry_clear() {
        let mut registry = GenerationProviderRegistry::new();
        registry
            .register("dalle".to_string(), create_test_provider("dalle"))
            .unwrap();
        registry
            .register("midjourney".to_string(), create_test_provider("mj"))
            .unwrap();

        assert_eq!(registry.len(), 2);

        registry.clear();

        assert!(registry.is_empty());
        assert_eq!(registry.len(), 0);
    }

    // === Iterator tests ===

    #[test]
    fn test_registry_iter() {
        let mut registry = GenerationProviderRegistry::new();
        registry
            .register("dalle".to_string(), create_test_provider("dalle"))
            .unwrap();
        registry
            .register("midjourney".to_string(), create_test_provider("mj"))
            .unwrap();

        let items: Vec<_> = registry.iter().collect();
        assert_eq!(items.len(), 2);
    }

    // === Usage tests ===

    #[tokio::test]
    async fn test_registry_provider_usage() {
        let mut registry = GenerationProviderRegistry::new();
        registry
            .register("dalle".to_string(), create_test_provider("dalle"))
            .unwrap();

        let provider = registry.get("dalle").unwrap();

        let request = GenerationRequest::image("A sunset over mountains");
        let result = provider.generate(request).await;

        assert!(result.is_ok());
        let output = result.unwrap();
        assert!(output.data.is_url());
    }

    #[test]
    fn test_registry_multiple_providers() {
        let mut registry = GenerationProviderRegistry::new();

        let dalle = create_test_provider("dalle");
        let mj = create_test_provider("midjourney");
        let runway = create_test_provider("runway");

        registry.register("dalle".to_string(), dalle).unwrap();
        registry.register("midjourney".to_string(), mj).unwrap();
        registry.register("runway".to_string(), runway).unwrap();

        assert_eq!(registry.len(), 3);
        assert!(registry.contains("dalle"));
        assert!(registry.contains("midjourney"));
        assert!(registry.contains("runway"));

        let dalle_provider = registry.get("dalle").unwrap();
        assert_eq!(dalle_provider.name(), "dalle");

        let mj_provider = registry.get("midjourney").unwrap();
        assert_eq!(mj_provider.name(), "midjourney");
    }

    #[test]
    fn test_registry_can_re_register_after_remove() {
        let mut registry = GenerationProviderRegistry::new();

        registry
            .register("dalle".to_string(), create_test_provider("dalle-v1"))
            .unwrap();

        // Remove the provider
        registry.remove("dalle");

        // Should be able to register again with the same name
        let result = registry.register("dalle".to_string(), create_test_provider("dalle-v2"));
        assert!(result.is_ok());

        let provider = registry.get("dalle").unwrap();
        assert_eq!(provider.name(), "dalle-v2");
    }
}
