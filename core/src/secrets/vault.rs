//! Encrypted secret vault
//!
//! File-based encrypted storage for secrets using AES-256-GCM.
//! Location: ~/.aleph/secrets.vault

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use tracing::{debug, info, warn};

use super::crypto::SecretsCrypto;
use super::types::{DecryptedSecret, EncryptedEntry, EntryMetadata, SecretError, VaultData};
use crate::config::Config;

/// Current vault format version.
const VAULT_VERSION: u32 = 1;

/// Encrypted secret vault backed by a file.
pub struct SecretVault {
    data: VaultData,
    crypto: SecretsCrypto,
    path: PathBuf,
}

impl SecretVault {
    /// Open or create a vault at the given path with the provided master key.
    pub fn open(path: impl Into<PathBuf>, master_key: &str) -> Result<Self, SecretError> {
        let path = path.into();
        let crypto = SecretsCrypto::new(master_key);

        let data = if path.exists() {
            debug!(path = %path.display(), "Loading existing vault");
            let bytes = std::fs::read(&path)?;
            bincode::deserialize(&bytes).map_err(|e| {
                SecretError::Serialization(format!("Failed to deserialize vault: {}", e))
            })?
        } else {
            debug!(path = %path.display(), "Creating new vault");
            VaultData {
                version: VAULT_VERSION,
                entries: HashMap::new(),
            }
        };

        Ok(Self { data, crypto, path })
    }

    /// Save vault to disk with atomic write.
    fn save(&self) -> Result<(), SecretError> {
        let bytes = bincode::serialize(&self.data)
            .map_err(|e| SecretError::Serialization(format!("Failed to serialize vault: {}", e)))?;

        // Ensure parent directory exists
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent)?;
        }

        // Atomic write: write to temp file, then rename
        let tmp_path = self.path.with_extension("vault.tmp");
        std::fs::write(&tmp_path, &bytes)?;
        std::fs::rename(&tmp_path, &self.path)?;

        debug!(path = %self.path.display(), entries = self.data.entries.len(), "Vault saved");
        Ok(())
    }

    /// Get and decrypt a secret by name.
    pub fn get(&self, name: &str) -> Result<DecryptedSecret, SecretError> {
        let entry = self
            .data
            .entries
            .get(name)
            .ok_or_else(|| SecretError::NotFound(name.to_string()))?;

        let plaintext = self
            .crypto
            .decrypt(&entry.ciphertext, &entry.nonce, &entry.salt)?;

        Ok(DecryptedSecret::new(plaintext))
    }

    /// Encrypt and store a secret.
    pub fn set(
        &mut self,
        name: &str,
        value: &str,
        metadata: EntryMetadata,
    ) -> Result<(), SecretError> {
        let encrypted = self.crypto.encrypt(value)?;
        let now = chrono::Utc::now().timestamp();

        let entry = EncryptedEntry {
            ciphertext: encrypted.ciphertext,
            nonce: encrypted.nonce,
            salt: encrypted.salt,
            created_at: self
                .data
                .entries
                .get(name)
                .map(|e| e.created_at)
                .unwrap_or(now),
            updated_at: now,
            metadata,
        };

        self.data.entries.insert(name.to_string(), entry);
        self.save()?;

        info!(name = name, "Secret stored in vault");
        Ok(())
    }

    /// Delete a secret by name.
    pub fn delete(&mut self, name: &str) -> Result<bool, SecretError> {
        let removed = self.data.entries.remove(name).is_some();
        if removed {
            self.save()?;
            info!(name = name, "Secret deleted from vault");
        }
        Ok(removed)
    }

    /// Check if a secret exists.
    pub fn exists(&self, name: &str) -> bool {
        self.data.entries.contains_key(name)
    }

    /// List all secret names (never values).
    pub fn list(&self) -> Vec<(String, &EntryMetadata)> {
        self.data
            .entries
            .iter()
            .map(|(name, entry)| (name.clone(), &entry.metadata))
            .collect()
    }

    /// Get the vault file path.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Get the number of entries.
    pub fn len(&self) -> usize {
        self.data.entries.len()
    }

    /// Check if vault is empty.
    pub fn is_empty(&self) -> bool {
        self.data.entries.is_empty()
    }

    /// Get the default vault path.
    pub fn default_path() -> PathBuf {
        crate::utils::paths::get_config_dir()
            .map(|d| d.join("secrets.vault"))
            .unwrap_or_else(|_| PathBuf::from("secrets.vault"))
    }
}

impl super::injection::SecretResolver for SecretVault {
    fn resolve(&self, name: &str) -> Result<super::types::DecryptedSecret, super::types::SecretError> {
        self.get(name)
    }
}

/// Resolve the master key from environment or return error.
pub fn resolve_master_key() -> Result<String, SecretError> {
    std::env::var("ALEPH_MASTER_KEY").map_err(|_| SecretError::MasterKeyMissing)
}

/// Resolve secret_name references in provider configs.
///
/// For each provider with a secret_name, decrypts the secret from vault
/// and populates the in-memory api_key field. The config.toml file is
/// never modified — this is a runtime-only operation.
pub fn resolve_provider_secrets(
    config: &mut Config,
    vault: &SecretVault,
) -> Result<(), SecretError> {
    for (name, provider) in config.providers.iter_mut() {
        if let Some(ref secret_name) = provider.secret_name {
            if provider.api_key.is_none() {
                match vault.get(secret_name) {
                    Ok(secret) => {
                        provider.api_key = Some(secret.expose().to_string());
                        debug!(
                            provider = %name,
                            secret_name = %secret_name,
                            "Resolved API key from vault"
                        );
                    }
                    Err(SecretError::NotFound(ref s)) => {
                        warn!(
                            provider = %name,
                            secret_name = %s,
                            "Secret not found in vault, provider will fail on use"
                        );
                    }
                    Err(e) => return Err(e),
                }
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::ProviderConfig;
    use tempfile::TempDir;

    fn test_vault(dir: &TempDir) -> SecretVault {
        let path = dir.path().join("test.vault");
        SecretVault::open(path, "test-master-key").unwrap()
    }

    #[test]
    fn test_set_and_get() {
        let dir = TempDir::new().unwrap();
        let mut vault = test_vault(&dir);

        vault
            .set("my_key", "sk-ant-secret", EntryMetadata::default())
            .unwrap();
        let secret = vault.get("my_key").unwrap();
        assert_eq!(secret.expose(), "sk-ant-secret");
    }

    #[test]
    fn test_get_not_found() {
        let dir = TempDir::new().unwrap();
        let vault = test_vault(&dir);

        let result = vault.get("nonexistent");
        assert!(matches!(result, Err(SecretError::NotFound(_))));
    }

    #[test]
    fn test_delete() {
        let dir = TempDir::new().unwrap();
        let mut vault = test_vault(&dir);

        vault
            .set("key1", "value1", EntryMetadata::default())
            .unwrap();
        assert!(vault.exists("key1"));

        let deleted = vault.delete("key1").unwrap();
        assert!(deleted);
        assert!(!vault.exists("key1"));
    }

    #[test]
    fn test_delete_nonexistent() {
        let dir = TempDir::new().unwrap();
        let mut vault = test_vault(&dir);

        let deleted = vault.delete("nonexistent").unwrap();
        assert!(!deleted);
    }

    #[test]
    fn test_list() {
        let dir = TempDir::new().unwrap();
        let mut vault = test_vault(&dir);

        vault
            .set(
                "key1",
                "v1",
                EntryMetadata {
                    provider: Some("anthropic".into()),
                    ..Default::default()
                },
            )
            .unwrap();
        vault.set("key2", "v2", EntryMetadata::default()).unwrap();

        let list = vault.list();
        assert_eq!(list.len(), 2);

        let names: Vec<&str> = list.iter().map(|(n, _)| n.as_str()).collect();
        assert!(names.contains(&"key1"));
        assert!(names.contains(&"key2"));
    }

    #[test]
    fn test_persistence_across_reopen() {
        let dir = TempDir::new().unwrap();
        let vault_path = dir.path().join("persist.vault");

        // Write
        {
            let mut vault = SecretVault::open(&vault_path, "master").unwrap();
            vault
                .set(
                    "persistent_key",
                    "persistent_value",
                    EntryMetadata::default(),
                )
                .unwrap();
        }

        // Read with same key
        {
            let vault = SecretVault::open(&vault_path, "master").unwrap();
            let secret = vault.get("persistent_key").unwrap();
            assert_eq!(secret.expose(), "persistent_value");
        }
    }

    #[test]
    fn test_wrong_master_key_on_reopen() {
        let dir = TempDir::new().unwrap();
        let vault_path = dir.path().join("wrongkey.vault");

        // Write with correct key
        {
            let mut vault = SecretVault::open(&vault_path, "correct-key").unwrap();
            vault
                .set("secret", "value", EntryMetadata::default())
                .unwrap();
        }

        // Try to read with wrong key
        {
            let vault = SecretVault::open(&vault_path, "wrong-key").unwrap();
            let result = vault.get("secret");
            assert!(matches!(result, Err(SecretError::DecryptionFailed)));
        }
    }

    #[test]
    fn test_overwrite_preserves_created_at() {
        let dir = TempDir::new().unwrap();
        let mut vault = test_vault(&dir);

        vault.set("key", "v1", EntryMetadata::default()).unwrap();
        let created = vault.data.entries.get("key").unwrap().created_at;

        // Overwrite
        vault.set("key", "v2", EntryMetadata::default()).unwrap();
        let new_created = vault.data.entries.get("key").unwrap().created_at;
        let updated = vault.data.entries.get("key").unwrap().updated_at;

        assert_eq!(created, new_created); // created_at preserved
        assert!(updated >= created);

        // Value updated
        assert_eq!(vault.get("key").unwrap().expose(), "v2");
    }

    #[test]
    fn test_len_and_is_empty() {
        let dir = TempDir::new().unwrap();
        let mut vault = test_vault(&dir);

        assert!(vault.is_empty());
        assert_eq!(vault.len(), 0);

        vault.set("key", "val", EntryMetadata::default()).unwrap();
        assert!(!vault.is_empty());
        assert_eq!(vault.len(), 1);
    }

    #[test]
    fn test_metadata_stored() {
        let dir = TempDir::new().unwrap();
        let mut vault = test_vault(&dir);

        vault
            .set(
                "key",
                "val",
                EntryMetadata {
                    description: Some("My Anthropic key".into()),
                    provider: Some("anthropic".into()),
                },
            )
            .unwrap();

        let list = vault.list();
        let (_, meta) = list.iter().find(|(n, _)| n == "key").unwrap();
        assert_eq!(meta.description.as_deref(), Some("My Anthropic key"));
        assert_eq!(meta.provider.as_deref(), Some("anthropic"));
    }

    #[test]
    fn test_resolve_provider_secrets() {
        let dir = TempDir::new().unwrap();
        let vault_path = dir.path().join("test.vault");
        let mut vault = SecretVault::open(&vault_path, "master").unwrap();

        // Store a secret
        vault
            .set("anthropic_key", "sk-ant-real-key", EntryMetadata::default())
            .unwrap();

        // Create config with secret_name reference
        let mut config = Config::default();
        let mut provider = ProviderConfig::test_config("claude-sonnet");
        provider.api_key = None;
        provider.secret_name = Some("anthropic_key".into());
        config.providers.insert("claude".into(), provider);

        // Resolve
        resolve_provider_secrets(&mut config, &vault).unwrap();

        // api_key should now be populated
        assert_eq!(
            config.providers.get("claude").unwrap().api_key.as_deref(),
            Some("sk-ant-real-key")
        );
    }
}
