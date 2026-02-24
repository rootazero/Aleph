//! Secret provider abstraction
//!
//! Defines the `SecretProvider` trait for pluggable secret backends
//! (local vault, 1Password, AWS Secrets Manager, etc.).

pub mod local_vault;

use async_trait::async_trait;
use chrono::{DateTime, Utc};

use super::types::{DecryptedSecret, SecretError};

/// Health status of a secret provider.
#[derive(Debug, Clone, PartialEq)]
pub enum ProviderStatus {
    /// Provider is ready to serve secrets.
    Ready,
    /// Provider requires authentication before use.
    NeedsAuth,
    /// Provider is not available (e.g., network down, CLI missing).
    Unavailable,
}

/// Metadata about a secret entry within a provider.
#[derive(Debug, Clone)]
pub struct SecretMetadata {
    /// Secret name / key.
    pub name: String,
    /// Provider that owns this secret.
    pub provider: String,
    /// When the secret was last updated.
    pub updated_at: DateTime<Utc>,
}

/// Trait for pluggable secret backends.
///
/// Each implementation encapsulates access to one secret source.
/// The `SecretRouter` (future) will dispatch `get()` calls to the
/// appropriate provider based on reference syntax.
#[async_trait]
pub trait SecretProvider: Send + Sync {
    /// Returns a human-readable provider type identifier (e.g., "local_vault", "1password").
    fn provider_type(&self) -> &str;

    /// Retrieve and decrypt a secret by reference.
    async fn get(&self, reference: &str) -> Result<DecryptedSecret, SecretError>;

    /// Check whether the provider is healthy and ready to serve.
    async fn health_check(&self) -> Result<ProviderStatus, SecretError>;

    /// List metadata for all secrets this provider can serve.
    async fn list(&self) -> Result<Vec<SecretMetadata>, SecretError>;
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use std::sync::Mutex;

    /// A simple in-memory mock provider for testing the trait surface.
    struct MockProvider {
        secrets: Mutex<HashMap<String, String>>,
        status: ProviderStatus,
    }

    impl MockProvider {
        fn new(status: ProviderStatus) -> Self {
            Self {
                secrets: Mutex::new(HashMap::new()),
                status,
            }
        }

        fn insert(&self, name: &str, value: &str) {
            self.secrets
                .lock()
                .unwrap()
                .insert(name.to_string(), value.to_string());
        }
    }

    #[async_trait]
    impl SecretProvider for MockProvider {
        fn provider_type(&self) -> &str {
            "mock"
        }

        async fn get(&self, reference: &str) -> Result<DecryptedSecret, SecretError> {
            let map = self.secrets.lock().unwrap();
            map.get(reference)
                .map(|v| DecryptedSecret::new(v.as_str()))
                .ok_or_else(|| SecretError::NotFound(reference.to_string()))
        }

        async fn health_check(&self) -> Result<ProviderStatus, SecretError> {
            Ok(self.status.clone())
        }

        async fn list(&self) -> Result<Vec<SecretMetadata>, SecretError> {
            let map = self.secrets.lock().unwrap();
            Ok(map
                .keys()
                .map(|k| SecretMetadata {
                    name: k.clone(),
                    provider: "mock".to_string(),
                    updated_at: Utc::now(),
                })
                .collect())
        }
    }

    #[tokio::test]
    async fn test_mock_get_existing() {
        let provider = MockProvider::new(ProviderStatus::Ready);
        provider.insert("api_key", "sk-secret-123");

        let secret = provider.get("api_key").await.unwrap();
        assert_eq!(secret.expose(), "sk-secret-123");
    }

    #[tokio::test]
    async fn test_mock_get_not_found() {
        let provider = MockProvider::new(ProviderStatus::Ready);

        let result = provider.get("nonexistent").await;
        assert!(matches!(result, Err(SecretError::NotFound(_))));
    }

    #[tokio::test]
    async fn test_mock_health_check() {
        let ready = MockProvider::new(ProviderStatus::Ready);
        assert_eq!(ready.health_check().await.unwrap(), ProviderStatus::Ready);

        let needs_auth = MockProvider::new(ProviderStatus::NeedsAuth);
        assert_eq!(
            needs_auth.health_check().await.unwrap(),
            ProviderStatus::NeedsAuth
        );
    }

    #[tokio::test]
    async fn test_mock_list() {
        let provider = MockProvider::new(ProviderStatus::Ready);
        provider.insert("key_a", "val_a");
        provider.insert("key_b", "val_b");

        let list = provider.list().await.unwrap();
        assert_eq!(list.len(), 2);

        let names: Vec<&str> = list.iter().map(|m| m.name.as_str()).collect();
        assert!(names.contains(&"key_a"));
        assert!(names.contains(&"key_b"));

        // All entries should report "mock" as provider
        for entry in &list {
            assert_eq!(entry.provider, "mock");
        }
    }
}
