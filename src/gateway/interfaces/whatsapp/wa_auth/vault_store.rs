//! Vault-backed WhatsApp auth storage

use crate::secrets::vault::SecretVault;
use serde::{Deserialize, Serialize};
use std::sync::{Arc, Mutex};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
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
}

impl WaAuthManager {
    pub fn new(account_id: impl Into<String>) -> Self {
        let path = crate::secrets::vault::SecretVault::default_path();
        let vault = SecretVault::open(&path).unwrap_or_else(|_| SecretVault::empty(path));
        Self::with_vault(vault, account_id)
    }

    pub fn with_vault(vault: SecretVault, account_id: impl Into<String>) -> Self {
        Self {
            vault: Arc::new(Mutex::new(vault)),
            account_id: account_id.into(),
        }
    }

    fn key(&self) -> String {
        format!("whatsapp/auth/{}", self.account_id)
    }

    pub fn save(&self, data: &WaAuthData) -> Result<(), WaAuthError> {
        let bytes =
            bincode::serialize(data).map_err(|e| WaAuthError::Serialization(e.to_string()))?;
        let entry = crate::secrets::types::EncryptedEntry {
            ciphertext: bytes,
            nonce: [0u8; 12],
            salt: [0u8; 32],
            created_at: chrono::Utc::now().timestamp(),
            updated_at: chrono::Utc::now().timestamp(),
            metadata: crate::secrets::types::EntryMetadata::default(),
        };
        let mut vault = self.vault.lock().unwrap();
        vault
            .set(&self.key(), entry)
            .map_err(|e| WaAuthError::Vault(e.to_string()))
    }

    pub fn load(&self) -> Result<WaAuthData, WaAuthError> {
        let vault = self.vault.lock().unwrap();
        let entry = vault
            .get(&self.key())
            .map_err(|_| WaAuthError::NotFound(self.account_id.clone()))?;
        let data: WaAuthData = bincode::deserialize(&entry.ciphertext)
            .map_err(|e| WaAuthError::Serialization(e.to_string()))?;
        Ok(data)
    }

    pub fn exists(&self) -> bool {
        let vault = self.vault.lock().unwrap();
        vault.exists(&self.key())
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
        let auth = WaAuthManager::with_vault(vault, "test_account");

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
        let auth = WaAuthManager::with_vault(vault, "missing_account");
        assert!(matches!(auth.load(), Err(WaAuthError::NotFound(_))));
    }
}
