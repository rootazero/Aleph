//! Auth Profile Provider Registry
//!
//! Integrates auth profile management with the provider registry system.
//! Provides automatic API key rotation based on profile status.
//!
//! # Architecture
//!
//! ```text
//! ┌─────────────────────────────────────────────────────────────┐
//! │              AuthProfileProviderRegistry                     │
//! ├─────────────────────────────────────────────────────────────┤
//! │  ┌────────────────┐  ┌────────────────┐  ┌────────────────┐ │
//! │  │ mock:key1 │  │ mock:key2 │  │ anthropic:key3 │ │
//! │  │   (primary)    │  │   (backup 1)   │  │   (backup 2)   │ │
//! │  └───────┬────────┘  └───────┬────────┘  └───────┬────────┘ │
//! │          │                   │                   │          │
//! │          └───────────────────┼───────────────────┘          │
//! │                              ▼                               │
//! │                    Profile Ordering                          │
//! │             (type > lastUsed > cooldown)                     │
//! └─────────────────────────────────────────────────────────────┘
//! ```
//!
//! # Usage
//!
//! ```rust,ignore
//! use alephcore::providers::auth_profile_registry::AuthProfileProviderRegistry;
//!
//! // Create registry from auth profile store
//! let store = AuthProfileStore::new();
//! let registry = AuthProfileProviderRegistry::new(store, "anthropic");
//!
//! // Get best available provider
//! let provider = registry.default_provider();
//!
//! // After API call success
//! registry.mark_success("mock:key1");
//!
//! // After API call failure
//! registry.mark_failure("mock:key1", AuthProfileFailureReason::RateLimit);
//! ```

use std::collections::HashMap;
use std::sync::{Arc, RwLock};
use tracing::{debug, info, warn};

use crate::config::ProviderConfig;
use crate::error::AlephError;
use crate::providers::{
    auth_profiles::{
        AuthProfileCredential, AuthProfileFailureReason, AuthProfileStore,
        CooldownConfig, mark_profile_failure, mark_profile_used, resolve_profile_order,
    },
    create_provider, AiProvider,
};
use crate::thinker::ProviderRegistry;

/// Configuration for auth profile registry
#[derive(Debug, Clone)]
pub struct AuthProfileRegistryConfig {
    /// Provider type (e.g., "anthropic", "openai")
    pub provider_type: String,
    /// Model to use
    pub model: String,
    /// Base URL override (optional)
    pub base_url: Option<String>,
    /// Timeout in seconds
    pub timeout_seconds: u64,
    /// Cooldown configuration
    pub cooldown: CooldownConfig,
}

impl Default for AuthProfileRegistryConfig {
    fn default() -> Self {
        Self {
            provider_type: "claude".to_string(),
            model: "claude-sonnet-4-20250514".to_string(),
            base_url: None,
            timeout_seconds: 300,
            cooldown: CooldownConfig::default(),
        }
    }
}

/// Provider registry that uses auth profiles for API key rotation
pub struct AuthProfileProviderRegistry {
    /// Auth profile store
    store: Arc<RwLock<AuthProfileStore>>,
    /// Pre-created providers by profile ID
    providers: Arc<RwLock<HashMap<String, Arc<dyn AiProvider>>>>,
    /// Target provider ID (e.g., "anthropic")
    target_provider: String,
    /// Configuration
    config: AuthProfileRegistryConfig,
    /// Current active profile ID
    active_profile: Arc<RwLock<Option<String>>>,
}

impl AuthProfileProviderRegistry {
    /// Create a new auth profile provider registry
    ///
    /// # Arguments
    ///
    /// * `store` - Auth profile store containing credentials
    /// * `config` - Registry configuration
    pub fn new(store: AuthProfileStore, config: AuthProfileRegistryConfig) -> Self {
        let target_provider = config.provider_type.clone();
        let registry = Self {
            store: Arc::new(RwLock::new(store)),
            providers: Arc::new(RwLock::new(HashMap::new())),
            target_provider,
            config,
            active_profile: Arc::new(RwLock::new(None)),
        };

        // Initialize providers
        registry.refresh_providers();

        registry
    }

    /// Refresh provider instances from the auth profile store
    pub fn refresh_providers(&self) {
        let store = self.store.read().unwrap();
        let profile_ids = store.list_profiles_for_provider(&self.target_provider);

        let mut providers = self.providers.write().unwrap();
        providers.clear();

        for profile_id in profile_ids {
            if let Some(cred) = store.profiles.get(&profile_id) {
                match self.create_provider_from_credential(cred) {
                    Ok(provider) => {
                        debug!(profile_id = %profile_id, "Created provider from auth profile");
                        providers.insert(profile_id, provider);
                    }
                    Err(e) => {
                        warn!(
                            profile_id = %profile_id,
                            error = %e,
                            "Failed to create provider from auth profile"
                        );
                    }
                }
            }
        }

        info!(
            provider = %self.target_provider,
            count = providers.len(),
            "Refreshed auth profile providers"
        );
    }

    /// Create a provider from a credential
    fn create_provider_from_credential(
        &self,
        cred: &AuthProfileCredential,
    ) -> Result<Arc<dyn AiProvider>, AlephError> {
        let api_key = cred.resolve_key().map(|s| s.to_string());

        if api_key.is_none() || api_key.as_ref().is_some_and(|k| k.is_empty()) {
            return Err(AlephError::invalid_config("Auth profile has no valid key"));
        }

        let provider_config = ProviderConfig {
            protocol: Some(self.config.provider_type.clone()),
            api_key,
            secret_name: None,
            model: self.config.model.clone(),
            base_url: self.config.base_url.clone(),
            color: "#d97757".to_string(), // Default Claude color
            timeout_seconds: self.config.timeout_seconds,
            enabled: true,
            max_tokens: Some(8192),
            temperature: Some(0.7),
            top_p: None,
            top_k: None,
            frequency_penalty: None,
            presence_penalty: None,
            stop_sequences: None,
            thinking_level: None,
            media_resolution: None,
            repeat_penalty: None,
            system_prompt_mode: None,
        };

        create_provider(&self.config.provider_type, provider_config)
    }

    /// Get the best available profile ID based on ordering
    fn get_best_profile(&self) -> Option<String> {
        let store = self.store.read().unwrap();
        let order = resolve_profile_order(&store, &self.target_provider, None, None);

        // Find first profile that has a provider
        let providers = self.providers.read().unwrap();
        order.into_iter().find(|id| providers.contains_key(id))
    }

    /// Mark a profile as successfully used
    pub fn mark_success(&self, profile_id: &str) {
        let mut store = self.store.write().unwrap();
        mark_profile_used(&mut store, profile_id);
        debug!(profile_id = %profile_id, "Marked profile as used");
    }

    /// Mark a profile as failed
    pub fn mark_failure(&self, profile_id: &str, reason: AuthProfileFailureReason) {
        let mut store = self.store.write().unwrap();
        mark_profile_failure(&mut store, profile_id, reason, &self.config.cooldown);
        warn!(
            profile_id = %profile_id,
            reason = ?reason,
            "Marked profile as failed"
        );
    }

    /// Get the currently active profile ID
    pub fn active_profile_id(&self) -> Option<String> {
        self.active_profile.read().unwrap().clone()
    }

    /// Get all available profile IDs
    pub fn available_profiles(&self) -> Vec<String> {
        let store = self.store.read().unwrap();
        resolve_profile_order(&store, &self.target_provider, None, None)
    }

    /// Get profile count
    pub fn profile_count(&self) -> usize {
        self.providers.read().unwrap().len()
    }

    /// Get a reference to the auth profile store
    pub fn store(&self) -> &Arc<RwLock<AuthProfileStore>> {
        &self.store
    }
}

impl ProviderRegistry for AuthProfileProviderRegistry {
    fn get(&self, _model: &crate::thinker::ModelId) -> Option<Arc<dyn AiProvider>> {
        // For now, ignore model routing and use default
        Some(self.default_provider())
    }

    fn default_provider(&self) -> Arc<dyn AiProvider> {
        // Get best profile
        let profile_id = self.get_best_profile();

        if let Some(ref id) = profile_id {
            let providers = self.providers.read().unwrap();
            if let Some(provider) = providers.get(id) {
                // Update active profile
                *self.active_profile.write().unwrap() = Some(id.clone());
                debug!(profile_id = %id, "Selected provider from auth profile");
                let result: Arc<dyn AiProvider> = provider.clone();
                return result;
            }
        }

        // Fallback: return first available provider
        let providers = self.providers.read().unwrap();
        if let Some((id, provider)) = providers.iter().next() {
            warn!(
                profile_id = %id,
                "Falling back to first available provider (no healthy profiles)"
            );
            *self.active_profile.write().unwrap() = Some(id.clone());
            let result: Arc<dyn AiProvider> = provider.clone();
            return result;
        }

        // No providers available - create a mock that errors
        warn!("No auth profile providers available, creating error provider");
        Arc::new(NoProfileProvider)
    }
}

/// Provider that returns an error when no profiles are available
struct NoProfileProvider;

impl AiProvider for NoProfileProvider {
    fn process(
        &self,
        _input: &str,
        _system_prompt: Option<&str>,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = crate::error::Result<String>> + Send + '_>> {
        Box::pin(async {
            Err(AlephError::provider(
                "No auth profiles configured. Please add an API key to ~/.aleph/auth-profiles.json"
            ))
        })
    }

    fn name(&self) -> &str {
        "no-profile"
    }

    fn color(&self) -> &str {
        "#ff0000"
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::providers::auth_profiles::{ApiKeyCredential, AuthProfileCredential};

    fn create_test_store() -> AuthProfileStore {
        let mut store = AuthProfileStore::new();

        // Add test profiles (using "mock" provider to match test config)
        store.upsert_profile(
            "mock:key1".to_string(),
            AuthProfileCredential::ApiKey(ApiKeyCredential {
                provider: "mock".to_string(),
                key: "sk-test-key-1".to_string(),
                email: None,
            }),
        );
        store.upsert_profile(
            "mock:key2".to_string(),
            AuthProfileCredential::ApiKey(ApiKeyCredential {
                provider: "mock".to_string(),
                key: "sk-test-key-2".to_string(),
                email: None,
            }),
        );

        store
    }

    #[test]
    fn test_registry_creation() {
        let store = create_test_store();
        let config = AuthProfileRegistryConfig {
            provider_type: "mock".to_string(), // Use mock to avoid real API calls
            ..Default::default()
        };

        let registry = AuthProfileProviderRegistry::new(store, config);

        // Should have created providers for both profiles
        assert_eq!(registry.profile_count(), 2);
    }

    #[test]
    fn test_available_profiles() {
        let store = create_test_store();
        let config = AuthProfileRegistryConfig {
            provider_type: "mock".to_string(),
            ..Default::default()
        };

        let registry = AuthProfileProviderRegistry::new(store, config);
        let profiles = registry.available_profiles();

        assert_eq!(profiles.len(), 2);
        assert!(profiles.contains(&"mock:key1".to_string()));
        assert!(profiles.contains(&"mock:key2".to_string()));
    }

    #[test]
    fn test_mark_success() {
        let store = create_test_store();
        let config = AuthProfileRegistryConfig {
            provider_type: "mock".to_string(),
            ..Default::default()
        };

        let registry = AuthProfileProviderRegistry::new(store, config);

        // Mark success
        registry.mark_success("mock:key1");

        // Check that lastUsed was updated
        let store = registry.store.read().unwrap();
        let stats = store.get_usage_stats("mock:key1");
        assert!(stats.is_some());
        assert!(stats.unwrap().last_used.is_some());
    }

    #[test]
    fn test_mark_failure() {
        let store = create_test_store();
        let config = AuthProfileRegistryConfig {
            provider_type: "mock".to_string(),
            ..Default::default()
        };

        let registry = AuthProfileProviderRegistry::new(store, config);

        // Mark failure
        registry.mark_failure("mock:key1", AuthProfileFailureReason::RateLimit);

        // Check that cooldown was set
        let store = registry.store.read().unwrap();
        let stats = store.get_usage_stats("mock:key1");
        assert!(stats.is_some());
        assert!(stats.unwrap().cooldown_until.is_some());
    }

    #[test]
    fn test_profile_rotation_after_failure() {
        let store = create_test_store();
        let config = AuthProfileRegistryConfig {
            provider_type: "mock".to_string(),
            ..Default::default()
        };

        let registry = AuthProfileProviderRegistry::new(store, config);

        // Get initial provider (should be one of the profiles)
        let _ = registry.default_provider();
        let first_profile = registry.active_profile_id();

        // Mark first profile as failed
        if let Some(ref id) = first_profile {
            registry.mark_failure(id, AuthProfileFailureReason::RateLimit);
        }

        // Get provider again - should be different profile
        let _ = registry.default_provider();
        let second_profile = registry.active_profile_id();

        // With only 2 profiles and one in cooldown, should switch
        // (though mock provider creation might fail, so we check both scenarios)
        if registry.profile_count() == 2 && first_profile.is_some() {
            assert_ne!(first_profile, second_profile);
        }
    }

    #[test]
    fn test_empty_store() {
        let store = AuthProfileStore::new();
        let config = AuthProfileRegistryConfig {
            provider_type: "mock".to_string(),
            ..Default::default()
        };

        let registry = AuthProfileProviderRegistry::new(store, config);

        assert_eq!(registry.profile_count(), 0);

        // Should return NoProfileProvider
        let provider = registry.default_provider();
        assert_eq!(provider.name(), "no-profile");
    }
}
