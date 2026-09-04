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
use crate::generation::{GenerationProvider, VoiceInfo};
use crate::sync_primitives::Arc;
use std::collections::HashMap;

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
    /// Canonical-name → config-name index. The primary table is keyed by the
    /// user's config section name (preserving multi-instance setups like
    /// `dalle-prod` / `dalle-dev`), but every provider also answers to its
    /// canonical [`GenerationProvider::name`] (e.g. `"openai-image"`). Log
    /// lines emitted by the provider itself carry the canonical name while
    /// registry-side logs carry the config name — this index is what lets
    /// the two be correlated during incident triage.
    canonical_index: HashMap<String, String>,
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
    #[must_use]
    pub fn new() -> Self {
        Self {
            providers: HashMap::new(),
            canonical_index: HashMap::new(),
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
            // A duplicate registration is a config bug (e.g. two
            // `[generation.openai_image]` sections), not an internal
            // failure — the dedicated variant maps to `invalid_config`
            // with actionable guidance instead of "please try again".
            return Err(GenerationError::DuplicateProvider { name });
        }
        // Index the canonical name so `get("openai-image")` resolves the
        // provider registered as config section `"dalle"`. First registration
        // of a canonical name wins: two config sections backed by the same
        // provider type (multi-instance) keep their primary entries, and the
        // canonical lookup deterministically resolves to the FIRST one
        // registered (with a warn so the ambiguity is observable).
        let canonical = provider.name().to_string();
        if canonical != name {
            match self.canonical_index.entry(canonical.clone()) {
                std::collections::hash_map::Entry::Vacant(e) => {
                    e.insert(name.clone());
                }
                std::collections::hash_map::Entry::Occupied(existing) => {
                    tracing::warn!(
                        canonical = %canonical,
                        kept = %existing.get(),
                        ignored = %name,
                        "canonical name already claimed by another config section; \
                         canonical lookup keeps the first registration"
                    );
                }
            }
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
    #[must_use]
    pub fn get(&self, name: &str) -> Option<Arc<dyn GenerationProvider>> {
        // Primary lookup by config section name; fall back to the canonical
        // name (e.g. a log line referencing "openai-image" can find the
        // provider registered as "dalle").
        self.providers.get(name).cloned().or_else(|| {
            self.canonical_index
                .get(name)
                .and_then(|config_name| self.providers.get(config_name).cloned())
        })
    }

    /// The config section name a canonical provider name resolves to, if any.
    /// Useful for log correlation: `canonical_name_of("openai-image")` answers
    /// `"dalle"` when the provider was registered under that section.
    ///
    /// Dead public API — zero callers anywhere in src/, tests/, interfaces/,
    /// desktop/. The accessor was added speculatively for incident-triage log
    /// correlation but no log emitter, debug tool, or admin RPC actually
    /// consults it. Demoted to `pub(crate)` so the canonical_index field still
    /// has its symmetric accessor for any future log-correlation tool inside
    /// the crate; external callers see no surface. (severed-wire audit
    /// 2026-09-04, sw-generation-1-1.)
    #[must_use]
    pub(crate) fn config_name_for_canonical(&self, canonical: &str) -> Option<&str> {
        self.canonical_index.get(canonical).map(String::as_str)
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
    pub(crate) fn get_or_err(&self, name: &str) -> GenerationResult<Arc<dyn GenerationProvider>> {
        self.get(name).ok_or_else(|| {
            GenerationError::internal(format!("Provider '{name}' not found in registry"))
        })
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
    #[must_use]
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
    #[must_use]
    pub fn contains(&self, name: &str) -> bool {
        self.providers.contains_key(name)
    }

    /// Get the number of registered providers
    #[must_use]
    pub fn len(&self) -> usize {
        self.providers.len()
    }

    /// Check if the registry is empty
    #[must_use]
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
    #[must_use]
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
    #[must_use]
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
        let removed = self.providers.remove(name)?;
        // Keep the canonical index consistent: drop the mapping only when it
        // points at the removed entry (a multi-instance sibling's mapping —
        // if any — was never claimed by this name in the first place).
        let canonical = removed.name().to_string();
        if self
            .canonical_index
            .get(&canonical)
            .is_some_and(|v| v == name)
        {
            self.canonical_index.remove(&canonical);
        }
        Some(removed)
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
        self.canonical_index.clear();
    }

    /// Get an iterator over all providers
    ///
    /// # Returns
    ///
    /// Iterator yielding (name, provider) pairs
    pub fn iter(&self) -> impl Iterator<Item = (&String, &Arc<dyn GenerationProvider>)> {
        self.providers.iter()
    }

    /// Get the list of voices available for a given provider
    ///
    /// # Arguments
    ///
    /// * `provider_id` - The provider name to query
    ///
    /// # Returns
    ///
    /// List of voices, or an empty vec if the provider is not found or has none.
    pub fn get_voices_for_provider(&self, provider_id: &str) -> Vec<VoiceInfo> {
        if let Some(provider) = self.get(provider_id) {
            let voices = provider.list_voices();
            if voices.is_empty() {
                tracing::debug!("Provider '{}' has no voices configured", provider_id);
            }
            return voices;
        }
        tracing::warn!("Provider '{}' not found in registry", provider_id);
        vec![]
    }

    /// Get the first provider that supports a generation type
    ///
    /// Useful for simple cases where you just need any provider for a type.
    /// When multiple providers support the type, the one with the
    /// lexicographically smallest name is returned, so the choice is
    /// deterministic regardless of `HashMap` iteration order.
    ///
    /// # Arguments
    ///
    /// * `gen_type` - The generation type to look for
    ///
    /// # Returns
    ///
    /// * `Some((name, provider))` - A provider that supports the type
    /// * `None` - No provider supports this type
    #[must_use]
    pub fn first_for_type(
        &self,
        gen_type: GenerationType,
    ) -> Option<(String, Arc<dyn GenerationProvider>)> {
        self.providers
            .iter()
            .filter(|(_, p)| p.supported_types().contains(&gen_type))
            .min_by(|(a, _), (b, _)| a.cmp(b))
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

        // Not `InternalError`: a duplicate registration is a config bug (two
        // sections claiming one name), and the dedicated variant is what maps
        // it to `invalid_config` with actionable guidance instead of "please
        // try again". Asserting the variant — not just `is_err()` — is what
        // keeps that distinction from silently regressing to the catch-all.
        match result {
            Err(GenerationError::DuplicateProvider { name }) => assert_eq!(name, "dalle"),
            other => panic!("Expected DuplicateProvider, got {other:?}"),
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
