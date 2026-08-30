//! Vault-backed `WhatsApp` auth storage

use crate::secrets::crypto::SecretsCrypto;
use crate::secrets::vault::SecretVault;
use crate::sync_primitives::{Arc, Mutex};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct WaAuthData {
    pub creds_blob: Vec<u8>,
    pub keys_blob: Vec<u8>,
    pub app_state_sync: Vec<u8>,
}

#[derive(Debug, thiserror::Error)]
pub enum WaAuthError {
    #[error("Auth not found for account {0}")]
    NotFound(String),
    #[error("Serialization failed: {0}")]
    Serialization(String),
    #[error("Vault error: {0}")]
    Vault(String),
}

pub struct WaAuthManager {
    vault: Arc<Mutex<SecretVault>>,
    account_id: String,
    crypto: Option<SecretsCrypto>,
}

impl WaAuthManager {
    pub fn new(account_id: impl Into<String>) -> Self {
        let path = crate::secrets::vault::SecretVault::default_path();
        let vault = SecretVault::open_or_backup(path);
        let crypto = crate::gateway::security::SharedTokenManager::global_crypto();
        Self::with_vault_and_crypto(vault, account_id, crypto)
    }

    pub fn with_vault(vault: SecretVault, account_id: impl Into<String>) -> Self {
        Self::with_vault_and_crypto(vault, account_id, None)
    }

    fn with_vault_and_crypto(
        vault: SecretVault,
        account_id: impl Into<String>,
        crypto: Option<SecretsCrypto>,
    ) -> Self {
        Self {
            vault: Arc::new(Mutex::new(vault)),
            account_id: account_id.into(),
            crypto,
        }
    }

    fn key(&self) -> String {
        format!("whatsapp/auth/{}", self.account_id)
    }

    pub fn save(&self, data: &WaAuthData) -> Result<(), WaAuthError> {
        let bytes =
            bincode::serialize(data).map_err(|e| WaAuthError::Serialization(e.to_string()))?;

        // Fail closed: WhatsApp auth credentials include Signal session state
        // and signed pre-keys. Writing them as plaintext bincode to the vault
        // would silently expose them to any attacker who can read the file.
        // The previous `nonce = [0u8; 12]` plaintext fallback is removed; the
        // shared-token manager must be installed before WhatsApp auth can be
        // persisted.
        let crypto = self.crypto.as_ref().ok_or_else(|| {
            WaAuthError::Serialization(
                "Cannot persist WhatsApp auth: no shared-token manager is installed, \
                 refusing to store plaintext credentials in the vault"
                    .into(),
            )
        })?;
        let encrypted = crypto
            .encrypt(&String::from_utf8_lossy(&bytes))
            .map_err(|e| WaAuthError::Serialization(format!("Encryption failed: {e}")))?;
        let entry = crate::secrets::types::EncryptedEntry {
            ciphertext: encrypted.ciphertext,
            nonce: encrypted.nonce,
            salt: encrypted.salt,
            created_at: chrono::Utc::now().timestamp(),
            updated_at: chrono::Utc::now().timestamp(),
            metadata: crate::secrets::types::EntryMetadata::default(),
        };

        let mut vault = self.vault.lock().unwrap_or_else(|e| e.into_inner());
        vault
            .set(&self.key(), entry)
            .map_err(|e| WaAuthError::Vault(e.to_string()))
    }

    pub fn load(&self) -> Result<WaAuthData, WaAuthError> {
        let vault = self.vault.lock().unwrap_or_else(|e| e.into_inner());
        let entry = vault.get(&self.key()).map_err(|e| match e {
            crate::secrets::types::SecretError::NotFound(_) => {
                WaAuthError::NotFound(self.account_id.clone())
            }
            _ => WaAuthError::Vault(e.to_string()),
        })?;

        // All persisted entries are encrypted by `save()`. A legacy entry
        // whose `nonce == [0u8; 12]` predates the fail-closed migration and
        // is refused rather than silently deserialised as plaintext.
        if entry.nonce == [0u8; 12] {
            return Err(WaAuthError::Serialization(
                "WhatsApp auth entry is in the legacy plaintext format; \
                 please re-authenticate after upgrading to rebuild the encrypted entry"
                    .into(),
            ));
        }
        let crypto = self.crypto.as_ref().ok_or_else(|| {
            WaAuthError::Serialization(
                "Encrypted WhatsApp auth data found but no crypto engine available".into(),
            )
        })?;
        let decrypted = crypto
            .decrypt(&entry.ciphertext, &entry.nonce, &entry.salt)
            .map_err(|e| WaAuthError::Serialization(format!("Decryption failed: {e}")))?;

        let data: WaAuthData = bincode::deserialize(&decrypted.into_bytes())
            .map_err(|e| WaAuthError::Serialization(e.to_string()))?;
        Ok(data)
    }

    #[must_use]
    pub fn exists(&self) -> bool {
        let vault = self.vault.lock().unwrap_or_else(|e| e.into_inner());
        vault.exists(&self.key())
    }

    /// Returns the `SQLite` database path for whatsapp-rust backend storage.
    #[must_use]
    pub fn db_path(&self) -> String {
        let base = dirs::data_dir().map_or_else(
            || std::env::temp_dir().join("aleph").join("whatsapp"),
            |p| p.join("aleph").join("whatsapp"),
        );
        let _ = std::fs::create_dir_all(&base);
        base.join(format!("auth_{}.db", self.account_id))
            .to_string_lossy()
            .to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::secrets::vault::SecretVault;
    use tempfile::TempDir;

    #[test]
    fn test_auth_roundtrip() {
        let dir = TempDir::new().unwrap();
        let vault = SecretVault::open(dir.path().join("test.vault")).unwrap();
        let crypto = crate::secrets::crypto::SecretsCrypto::new("test-master-key");
        let auth = WaAuthManager::with_vault_and_crypto(vault, "test_account", Some(crypto));

        let data = WaAuthData {
            creds_blob: vec![1, 2, 3],
            keys_blob: vec![4, 5, 6],
            app_state_sync: vec![7, 8, 9],
        };

        auth.save(&data).unwrap();
        let loaded = auth.load().unwrap();
        assert_eq!(loaded.creds_blob, data.creds_blob);
        assert_eq!(loaded.keys_blob, data.keys_blob);
    }

    #[test]
    fn test_auth_not_found() {
        let dir = TempDir::new().unwrap();
        let vault = SecretVault::open(dir.path().join("test.vault")).unwrap();
        let crypto = crate::secrets::crypto::SecretsCrypto::new("test-master-key");
        let auth = WaAuthManager::with_vault_and_crypto(vault, "missing_account", Some(crypto));
        assert!(matches!(auth.load(), Err(WaAuthError::NotFound(_))));
    }

    #[test]
    fn test_auth_save_fails_closed_without_crypto() {
        // The shared-token manager may not be installed at boot or in test
        // binaries. save() must refuse rather than persist plaintext.
        let dir = TempDir::new().unwrap();
        let vault = SecretVault::open(dir.path().join("test.vault")).unwrap();
        let auth = WaAuthManager::with_vault(vault, "no_crypto_account");
        let data = WaAuthData {
            creds_blob: vec![1],
            keys_blob: vec![2],
            app_state_sync: vec![3],
        };
        let err = auth.save(&data).expect_err("save must fail without crypto");
        assert!(matches!(err, WaAuthError::Serialization(_)));
    }
}
