//! Local vault secret provider
//!
//! Wraps `SecretVault` to implement the `SecretProvider` trait,
//! enabling the local encrypted vault to participate in the
//! provider-based secret routing system.

use crate::sync_primitives::RwLock;

use async_trait::async_trait;
use chrono::Utc;

use super::{ProviderStatus, SecretMetadata, SecretProvider};
use crate::secrets::types::{DecryptedSecret, SecretError};
use crate::secrets::vault::SecretVault;

/// Secret provider backed by the local encrypted vault.
///
/// Wraps a `SecretVault` in an `RwLock` so it can be shared across
/// async tasks while satisfying the `Send + Sync` bounds of `SecretProvider`.
///
/// Uses `std::sync::RwLock` rather than `tokio::sync::RwLock` because all
/// `SecretVault` operations are in-memory (data loaded at open time).
/// Lock hold times are sub-microsecond HashMap lookups.
pub struct LocalVaultProvider {
    vault: RwLock<SecretVault>,
}

impl LocalVaultProvider {
    /// Create a new provider wrapping an existing vault.
    pub fn new(vault: SecretVault) -> Self {
        Self {
            vault: RwLock::new(vault),
        }
    }
}

#[async_trait]
impl SecretProvider for LocalVaultProvider {
    fn provider_type(&self) -> &str {
        "local_vault"
    }

    async fn get(&self, reference: &str) -> Result<DecryptedSecret, SecretError> {
        let vault = self.vault.read().map_err(|e| SecretError::ProviderError {
            provider: "local_vault".into(),
            message: format!("Failed to acquire read lock: {}", e),
        })?;
        vault.get(reference)
    }

    async fn health_check(&self) -> Result<ProviderStatus, SecretError> {
        // The local vault is always ready once opened.
        Ok(ProviderStatus::Ready)
    }

    async fn list(&self) -> Result<Vec<SecretMetadata>, SecretError> {
        let vault = self.vault.read().map_err(|e| SecretError::ProviderError {
            provider: "local_vault".into(),
            message: format!("Failed to acquire read lock: {}", e),
        })?;

        let entries = vault.list();
        Ok(entries
            .into_iter()
            .map(|(name, meta)| SecretMetadata {
                name,
                provider: meta
                    .provider
                    .clone()
                    .unwrap_or_else(|| "local_vault".to_string()),
                // EntryMetadata doesn't expose updated_at through the public API,
                // so we use current time as an approximation.
                updated_at: Utc::now(),
            })
            .collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::secrets::types::EntryMetadata;
    use tempfile::TempDir;

    fn setup_provider() -> (TempDir, LocalVaultProvider) {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("test.vault");
        let vault = SecretVault::open(path, "test-master-key").unwrap();
        (dir, LocalVaultProvider::new(vault))
    }

    fn setup_provider_with_entries() -> (TempDir, LocalVaultProvider) {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("test.vault");
        let mut vault = SecretVault::open(path, "test-master-key").unwrap();
        vault
            .set(
                "anthropic_key",
                "sk-ant-secret",
                EntryMetadata {
                    description: Some("Anthropic API key".into()),
                    provider: Some("anthropic".into()),
                },
            )
            .unwrap();
        vault
            .set(
                "openai_key",
                "sk-openai-secret",
                EntryMetadata {
                    description: Some("OpenAI API key".into()),
                    provider: Some("openai".into()),
                },
            )
            .unwrap();
        (dir, LocalVaultProvider::new(vault))
    }

    #[tokio::test]
    async fn test_get() {
        let (_dir, provider) = setup_provider_with_entries();

        let secret = provider.get("anthropic_key").await.unwrap();
        assert_eq!(secret.expose(), "sk-ant-secret");
    }

    #[tokio::test]
    async fn test_get_not_found() {
        let (_dir, provider) = setup_provider();

        let result = provider.get("nonexistent").await;
        assert!(matches!(result, Err(SecretError::NotFound(_))));
    }

    #[tokio::test]
    async fn test_health_check() {
        let (_dir, provider) = setup_provider();

        let status = provider.health_check().await.unwrap();
        assert_eq!(status, ProviderStatus::Ready);
    }

    #[tokio::test]
    async fn test_list() {
        let (_dir, provider) = setup_provider_with_entries();

        let list = provider.list().await.unwrap();
        assert_eq!(list.len(), 2);

        let names: Vec<&str> = list.iter().map(|m| m.name.as_str()).collect();
        assert!(names.contains(&"anthropic_key"));
        assert!(names.contains(&"openai_key"));

        // Verify provider metadata is preserved
        let anthropic = list.iter().find(|m| m.name == "anthropic_key").unwrap();
        assert_eq!(anthropic.provider, "anthropic");
    }

    #[tokio::test]
    async fn test_provider_type() {
        let (_dir, provider) = setup_provider();

        assert_eq!(provider.provider_type(), "local_vault");
    }
}
