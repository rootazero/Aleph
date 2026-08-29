//! `AsyncSecretResolver` impl backed by `SharedTokenManager`.

use async_trait::async_trait;

use crate::gateway::security::shared_token::SharedTokenManager;
use crate::secrets::injection::AsyncSecretResolver;
use crate::secrets::types::{DecryptedSecret, SecretError};
use crate::sync_primitives::Arc;

/// `AsyncSecretResolver` impl that resolves secret names via the
/// `SharedTokenManager`-managed vault. The only production resolver.
pub struct VaultSecretResolver {
    inner: Arc<SharedTokenManager>,
}

impl VaultSecretResolver {
    pub const fn new(inner: Arc<SharedTokenManager>) -> Self {
        Self { inner }
    }
}

#[async_trait]
impl AsyncSecretResolver for VaultSecretResolver {
    async fn resolve(&self, name: &str) -> Result<DecryptedSecret, SecretError> {
        match self.inner.get_secret(name) {
            Ok(Some(decrypted)) => Ok(decrypted),
            Ok(None) => Err(SecretError::NotFound(name.to_string())),
            Err(e) => Err(SecretError::Io(std::io::Error::other(format!(
                "vault resolve error: {e}"
            )))),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gateway::security::shared_token::SharedTokenManager;
    use crate::gateway::security::store::SecurityStore;
    use tempfile::TempDir;

    fn make_mgr() -> (Arc<SharedTokenManager>, TempDir) {
        let tmp = TempDir::new().unwrap();
        let store = Arc::new(SecurityStore::in_memory().unwrap());
        let vault_path = tmp.path().join("vault");
        let mgr = SharedTokenManager::new(store, vault_path);
        // Seed shared-token secret bytes so encryption works.
        let _ = mgr.generate_token();
        (Arc::new(mgr), tmp)
    }

    #[tokio::test]
    async fn resolve_returns_decrypted_value_when_present() {
        let (mgr, _tmp) = make_mgr();
        mgr.store_secret("alpha", "sk-test-VALUE").unwrap();
        let resolver = VaultSecretResolver::new(mgr.clone());
        let decrypted = resolver.resolve("alpha").await.unwrap();
        assert_eq!(decrypted.expose(), "sk-test-VALUE");
    }

    #[tokio::test]
    async fn resolve_returns_not_found_for_missing_name() {
        let (mgr, _tmp) = make_mgr();
        let resolver = VaultSecretResolver::new(mgr);
        let err = resolver.resolve("ghost").await.unwrap_err();
        assert!(matches!(err, SecretError::NotFound(name) if name == "ghost"));
    }
}
