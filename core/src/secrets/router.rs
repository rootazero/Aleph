//! Secret router — dispatches resolution requests to the right provider.
//!
//! The [`SecretRouter`] maps logical secret names to concrete providers
//! using [`SecretMapping`] entries from the configuration. It respects
//! sensitivity levels: *Standard* secrets are cached with a TTL while
//! *High* sensitivity secrets always bypass the cache.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;

use crate::config::types::secrets::{SecretMapping, Sensitivity};
use crate::secrets::cache::SecretCache;
use crate::secrets::provider::SecretProvider;
use crate::secrets::types::{DecryptedSecret, SecretError};

// =============================================================================
// AsyncSecretResolver trait
// =============================================================================

/// Async interface for resolving a secret by logical name.
///
/// This is the primary consumption API — callers ask for a secret by name
/// and the implementation decides which provider to query, whether to use
/// the cache, etc.
#[async_trait]
pub trait AsyncSecretResolver: Send + Sync {
    /// Resolve a secret by its logical name.
    async fn resolve(&self, name: &str) -> Result<DecryptedSecret, SecretError>;
}

// =============================================================================
// SecretRouter
// =============================================================================

/// Routes secret resolution requests to the appropriate provider.
///
/// Resolution flow:
/// 1. Look up [`SecretMapping`] by name.
/// 2. If found with [`Sensitivity::Standard`]: check cache first, on miss
///    call the provider and cache the result.
/// 3. If found with [`Sensitivity::High`]: call the provider directly,
///    **never** cache.
/// 4. If no mapping exists: fall back to the default provider using the
///    secret name as the reference.
/// 5. If `mapping.reference` is `None`, the secret name is used as the
///    provider reference.
pub struct SecretRouter {
    mappings: HashMap<String, SecretMapping>,
    providers: HashMap<String, Arc<dyn SecretProvider>>,
    cache: SecretCache,
    default_provider: String,
}

impl SecretRouter {
    /// Create a new router.
    ///
    /// # Arguments
    /// * `mappings` — logical-name → provider+reference mappings
    /// * `providers` — provider-name → provider implementation
    /// * `default_provider` — provider name used for unmapped secrets
    pub fn new(
        mappings: HashMap<String, SecretMapping>,
        providers: HashMap<String, Arc<dyn SecretProvider>>,
        default_provider: String,
    ) -> Self {
        Self {
            mappings,
            providers,
            cache: SecretCache::new(),
            default_provider,
        }
    }

    /// Look up a provider by name, returning `ProviderNotFound` on miss.
    fn get_provider(&self, key: &str) -> Result<&Arc<dyn SecretProvider>, SecretError> {
        self.providers
            .get(key)
            .ok_or_else(|| SecretError::ProviderNotFound {
                provider: key.to_string(),
            })
    }
}

#[async_trait]
impl AsyncSecretResolver for SecretRouter {
    async fn resolve(&self, name: &str) -> Result<DecryptedSecret, SecretError> {
        if let Some(mapping) = self.mappings.get(name) {
            let provider = self.get_provider(&mapping.provider)?;
            let reference = mapping
                .reference
                .as_deref()
                .unwrap_or(name);

            match mapping.sensitivity {
                Sensitivity::Standard => {
                    // Check cache first
                    if let Some(cached) = self.cache.get(name).await {
                        tracing::debug!(secret = name, "cache hit");
                        return Ok(cached);
                    }

                    tracing::debug!(secret = name, provider = %mapping.provider, "cache miss — fetching from provider");
                    let secret = provider.get(reference).await?;

                    // Cache the result with the configured TTL
                    let ttl = Duration::from_secs(mapping.ttl);
                    // Rebuild because DecryptedSecret doesn't impl Clone
                    let to_cache = DecryptedSecret::new(secret.expose());
                    self.cache.put(name.to_string(), to_cache, ttl).await;

                    Ok(secret)
                }
                Sensitivity::High => {
                    tracing::debug!(
                        secret = name,
                        provider = %mapping.provider,
                        "high sensitivity — bypassing cache"
                    );
                    provider.get(reference).await
                }
            }
        } else {
            // No mapping — fall back to the default provider using name as reference
            tracing::debug!(secret = name, provider = %self.default_provider, "no mapping — falling back to default provider");
            let provider = self.get_provider(&self.default_provider)?;
            provider.get(name).await
        }
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::secrets::provider::{ProviderStatus, SecretMetadata};
    use chrono::Utc;

    // -------------------------------------------------------------------------
    // InMemoryProvider — lightweight test helper
    // -------------------------------------------------------------------------

    struct InMemoryProvider {
        secrets: HashMap<String, String>,
        name: String,
    }

    impl InMemoryProvider {
        fn new(name: &str, entries: Vec<(&str, &str)>) -> Self {
            Self {
                secrets: entries
                    .into_iter()
                    .map(|(k, v)| (k.to_string(), v.to_string()))
                    .collect(),
                name: name.to_string(),
            }
        }
    }

    #[async_trait]
    impl SecretProvider for InMemoryProvider {
        fn provider_type(&self) -> &str {
            &self.name
        }

        async fn get(&self, reference: &str) -> Result<DecryptedSecret, SecretError> {
            self.secrets
                .get(reference)
                .map(|v| DecryptedSecret::new(v.as_str()))
                .ok_or_else(|| SecretError::NotFound(reference.to_string()))
        }

        async fn health_check(&self) -> Result<ProviderStatus, SecretError> {
            Ok(ProviderStatus::Ready)
        }

        async fn list(&self) -> Result<Vec<SecretMetadata>, SecretError> {
            Ok(self
                .secrets
                .keys()
                .map(|k| SecretMetadata {
                    name: k.clone(),
                    provider: self.name.clone(),
                    updated_at: Utc::now(),
                })
                .collect())
        }
    }

    // -------------------------------------------------------------------------
    // Helper to build a router quickly
    // -------------------------------------------------------------------------

    fn make_router(
        mappings: Vec<(&str, SecretMapping)>,
        providers: Vec<(&str, Arc<dyn SecretProvider>)>,
        default: &str,
    ) -> SecretRouter {
        SecretRouter::new(
            mappings
                .into_iter()
                .map(|(k, v)| (k.to_string(), v))
                .collect(),
            providers
                .into_iter()
                .map(|(k, v)| (k.to_string(), v))
                .collect(),
            default.to_string(),
        )
    }

    // -------------------------------------------------------------------------
    // 1. resolve_mapped_local — standard mapping resolves correctly
    // -------------------------------------------------------------------------

    #[tokio::test]
    async fn resolve_mapped_local() {
        let local = Arc::new(InMemoryProvider::new(
            "local",
            vec![("my_api_key", "sk-local-123")],
        )) as Arc<dyn SecretProvider>;

        let router = make_router(
            vec![(
                "MY_API_KEY",
                SecretMapping {
                    provider: "local".into(),
                    reference: Some("my_api_key".into()),
                    sensitivity: Sensitivity::Standard,
                    ttl: 3600,
                },
            )],
            vec![("local", local)],
            "local",
        );

        let secret = router.resolve("MY_API_KEY").await.unwrap();
        assert_eq!(secret.expose(), "sk-local-123");
    }

    // -------------------------------------------------------------------------
    // 2. resolve_mapped_external — routes to correct provider
    // -------------------------------------------------------------------------

    #[tokio::test]
    async fn resolve_mapped_external() {
        let local = Arc::new(InMemoryProvider::new(
            "local",
            vec![("fallback_key", "local-val")],
        )) as Arc<dyn SecretProvider>;

        let op = Arc::new(InMemoryProvider::new(
            "1password",
            vec![("OpenAI/api-key", "sk-op-456")],
        )) as Arc<dyn SecretProvider>;

        let router = make_router(
            vec![(
                "OPENAI_KEY",
                SecretMapping {
                    provider: "op".into(),
                    reference: Some("OpenAI/api-key".into()),
                    sensitivity: Sensitivity::Standard,
                    ttl: 1800,
                },
            )],
            vec![("local", local), ("op", op)],
            "local",
        );

        let secret = router.resolve("OPENAI_KEY").await.unwrap();
        assert_eq!(secret.expose(), "sk-op-456");
    }

    // -------------------------------------------------------------------------
    // 3. resolve_unmapped_falls_back — unmapped name goes to default provider
    // -------------------------------------------------------------------------

    #[tokio::test]
    async fn resolve_unmapped_falls_back() {
        let local = Arc::new(InMemoryProvider::new(
            "local",
            vec![("SOME_TOKEN", "tok-default-789")],
        )) as Arc<dyn SecretProvider>;

        let router = make_router(vec![], vec![("local", local)], "local");

        let secret = router.resolve("SOME_TOKEN").await.unwrap();
        assert_eq!(secret.expose(), "tok-default-789");
    }

    // -------------------------------------------------------------------------
    // 4. resolve_unmapped_not_found — unmapped name, not in default → NotFound
    // -------------------------------------------------------------------------

    #[tokio::test]
    async fn resolve_unmapped_not_found() {
        let local = Arc::new(InMemoryProvider::new("local", vec![])) as Arc<dyn SecretProvider>;

        let router = make_router(vec![], vec![("local", local)], "local");

        let err = router.resolve("DOES_NOT_EXIST").await.unwrap_err();
        assert!(
            matches!(err, SecretError::NotFound(ref name) if name == "DOES_NOT_EXIST"),
            "expected NotFound, got: {err:?}"
        );
    }

    // -------------------------------------------------------------------------
    // 5. resolve_unknown_provider — mapping references non-existent provider
    // -------------------------------------------------------------------------

    #[tokio::test]
    async fn resolve_unknown_provider() {
        let local = Arc::new(InMemoryProvider::new("local", vec![])) as Arc<dyn SecretProvider>;

        let router = make_router(
            vec![(
                "SECRET_X",
                SecretMapping {
                    provider: "bitwarden".into(),
                    reference: Some("item/field".into()),
                    sensitivity: Sensitivity::Standard,
                    ttl: 3600,
                },
            )],
            vec![("local", local)],
            "local",
        );

        let err = router.resolve("SECRET_X").await.unwrap_err();
        assert!(
            matches!(err, SecretError::ProviderNotFound { ref provider } if provider == "bitwarden"),
            "expected ProviderNotFound(bitwarden), got: {err:?}"
        );
    }

    // -------------------------------------------------------------------------
    // 6. standard_caching — second resolve should come from cache
    // -------------------------------------------------------------------------

    #[tokio::test]
    async fn standard_caching() {
        let local = Arc::new(InMemoryProvider::new(
            "local",
            vec![("cached_key", "cached-value")],
        )) as Arc<dyn SecretProvider>;

        let router = make_router(
            vec![(
                "CACHED",
                SecretMapping {
                    provider: "local".into(),
                    reference: Some("cached_key".into()),
                    sensitivity: Sensitivity::Standard,
                    ttl: 3600,
                },
            )],
            vec![("local", local)],
            "local",
        );

        // First resolve — populates cache
        let first = router.resolve("CACHED").await.unwrap();
        assert_eq!(first.expose(), "cached-value");

        // Second resolve — should succeed (from cache) with the same value
        let second = router.resolve("CACHED").await.unwrap();
        assert_eq!(second.expose(), "cached-value");

        // Verify it's actually in the cache
        let from_cache = router.cache.get("CACHED").await;
        assert!(from_cache.is_some(), "expected entry to be in the cache");
        assert_eq!(from_cache.unwrap().expose(), "cached-value");
    }

    // -------------------------------------------------------------------------
    // 7. high_sensitivity_no_cache — high sensitivity should NOT be cached
    // -------------------------------------------------------------------------

    #[tokio::test]
    async fn high_sensitivity_no_cache() {
        let local = Arc::new(InMemoryProvider::new(
            "local",
            vec![("bank_password", "super-secret")],
        )) as Arc<dyn SecretProvider>;

        let router = make_router(
            vec![(
                "BANK_PW",
                SecretMapping {
                    provider: "local".into(),
                    reference: Some("bank_password".into()),
                    sensitivity: Sensitivity::High,
                    ttl: 0,
                },
            )],
            vec![("local", local)],
            "local",
        );

        // Resolve should succeed
        let secret = router.resolve("BANK_PW").await.unwrap();
        assert_eq!(secret.expose(), "super-secret");

        // Cache should NOT contain the entry
        let from_cache = router.cache.get("BANK_PW").await;
        assert!(
            from_cache.is_none(),
            "high-sensitivity secret should not be cached"
        );
    }
}
