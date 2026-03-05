//! Encrypted secret vault
//!
//! File-based encrypted storage for secrets using AES-256-GCM.
//! Location: ~/.aleph/secrets.vault

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use serde::{Deserialize, Serialize};
use tracing::{debug, info, warn};

use super::crypto::SecretsCrypto;
use super::types::{DecryptedSecret, EncryptedEntry, EntryMetadata, SecretError, VaultData};
use crate::config::Config;

/// Current vault format version.
const VAULT_VERSION: u32 = 1;

/// Keyring service name for Aleph vault.
const KEYRING_SERVICE: &str = "aleph.vault";
/// Keyring account name for the master key.
const KEYRING_ACCOUNT: &str = "master_key";

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

        // Restrict vault file permissions on Unix (owner-only read/write)
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let perms = std::fs::Permissions::from_mode(0o600);
            let _ = std::fs::set_permissions(&self.path, perms);
        }

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

#[async_trait::async_trait]
impl super::router::AsyncSecretResolver for SecretVault {
    async fn resolve(&self, name: &str) -> Result<super::types::DecryptedSecret, super::types::SecretError> {
        self.get(name)
    }
}

/// Resolve the master key from environment variable or OS keychain.
///
/// Priority:
/// 1. `ALEPH_MASTER_KEY` environment variable (CI / server override)
/// 2. OS keychain (macOS Keychain / Windows Credential Manager / Linux Secret Service)
pub fn resolve_master_key() -> Result<String, SecretError> {
    // Priority 1: env var (CI / server override)
    if let Ok(key) = std::env::var("ALEPH_MASTER_KEY") {
        if !key.is_empty() {
            return Ok(key);
        }
    }
    // Priority 2: OS keychain
    match get_master_key_from_keyring() {
        Ok(Some(key)) => return Ok(key),
        Ok(None) => {}
        Err(e) => tracing::debug!(error = %e, "Keychain unavailable, skipping"),
    }
    Err(SecretError::MasterKeyMissing)
}

/// Read the master key from the OS keychain.
///
/// Returns `Ok(Some(key))` if found, `Ok(None)` if not set,
/// or `Err` if the keychain backend is unavailable.
pub fn get_master_key_from_keyring() -> Result<Option<String>, SecretError> {
    let entry = keyring::Entry::new(KEYRING_SERVICE, KEYRING_ACCOUNT)
        .map_err(|e| SecretError::ProviderError {
            provider: "keychain".into(),
            message: format!("Failed to create keyring entry: {}", e),
        })?;

    match entry.get_password() {
        Ok(password) => Ok(Some(password)),
        Err(keyring::Error::NoEntry) => Ok(None),
        Err(e) => Err(SecretError::ProviderError {
            provider: "keychain".into(),
            message: format!("Failed to read from keychain: {}", e),
        }),
    }
}

/// Store the master key in the OS keychain.
pub fn store_master_key_to_keyring(key: &str) -> Result<(), SecretError> {
    let entry = keyring::Entry::new(KEYRING_SERVICE, KEYRING_ACCOUNT)
        .map_err(|e| SecretError::ProviderError {
            provider: "keychain".into(),
            message: format!("Failed to create keyring entry: {}", e),
        })?;

    entry.set_password(key).map_err(|e| SecretError::ProviderError {
        provider: "keychain".into(),
        message: format!("Failed to store in keychain: {}", e),
    })?;

    info!("Master key stored in OS keychain");
    Ok(())
}

/// Delete the master key from the OS keychain.
///
/// Returns `Ok(true)` if deleted, `Ok(false)` if not found.
pub fn delete_master_key_from_keyring() -> Result<bool, SecretError> {
    let entry = keyring::Entry::new(KEYRING_SERVICE, KEYRING_ACCOUNT)
        .map_err(|e| SecretError::ProviderError {
            provider: "keychain".into(),
            message: format!("Failed to create keyring entry: {}", e),
        })?;

    match entry.delete_credential() {
        Ok(()) => {
            info!("Master key removed from OS keychain");
            Ok(true)
        }
        Err(keyring::Error::NoEntry) => Ok(false),
        Err(e) => Err(SecretError::ProviderError {
            provider: "keychain".into(),
            message: format!("Failed to delete from keychain: {}", e),
        }),
    }
}

/// Current vault configuration status (never exposes the key value).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VaultStatus {
    /// Whether the vault file exists on disk.
    pub vault_exists: bool,
    /// Whether a master key is resolvable (env or keychain).
    pub master_key_configured: bool,
    /// Source of the master key: "env_var", "keychain", or null.
    pub master_key_source: Option<String>,
    /// Path to the vault file.
    pub vault_path: String,
    /// Whether the OS keychain backend is available.
    pub keychain_available: bool,
    /// Number of providers with plaintext API keys (candidates for migration).
    #[serde(default)]
    pub plaintext_key_count: usize,
    /// Number of providers with encrypted vault references.
    #[serde(default)]
    pub encrypted_key_count: usize,
}

/// Get the current vault status without exposing any secrets.
pub fn vault_status() -> VaultStatus {
    let vault_path = SecretVault::default_path();
    let vault_exists = vault_path.exists();

    // Check env var
    let env_key = std::env::var("ALEPH_MASTER_KEY")
        .ok()
        .filter(|k| !k.is_empty());

    // Check keychain
    let keychain_result = get_master_key_from_keyring();
    let keychain_available = keychain_result.is_ok();
    let keychain_key = keychain_result.ok().flatten();

    let (master_key_configured, master_key_source) = if env_key.is_some() {
        (true, Some("env_var".to_string()))
    } else if keychain_key.is_some() {
        (true, Some("keychain".to_string()))
    } else {
        (false, None)
    };

    let config = Config::load().ok();
    let plaintext_key_count = config
        .as_ref()
        .map(|c| c.providers.values().filter(|p| p.api_key.is_some()).count())
        .unwrap_or(0);
    let encrypted_key_count = config
        .as_ref()
        .map(|c| c.providers.values().filter(|p| p.secret_name.is_some()).count())
        .unwrap_or(0);

    VaultStatus {
        vault_exists,
        master_key_configured,
        master_key_source,
        vault_path: vault_path.display().to_string(),
        keychain_available,
        plaintext_key_count,
        encrypted_key_count,
    }
}

/// Resolve secret_name references in provider configs using an async resolver.
///
/// For each provider with a secret_name, resolves the secret via the resolver
/// and populates the in-memory api_key field. The config.toml file is
/// never modified — this is a runtime-only operation.
pub async fn resolve_provider_secrets(
    config: &mut Config,
    resolver: &dyn super::router::AsyncSecretResolver,
) -> Result<(), SecretError> {
    for (name, provider) in config.providers.iter_mut() {
        if let Some(ref secret_name) = provider.secret_name {
            if provider.api_key.is_none() {
                match resolver.resolve(secret_name).await {
                    Ok(secret) => {
                        provider.api_key = Some(secret.expose().to_string());
                        debug!(
                            provider = %name,
                            secret_name = %secret_name,
                            "Resolved API key from secret provider"
                        );
                    }
                    Err(SecretError::NotFound(ref s)) => {
                        warn!(
                            provider = %name,
                            secret_name = %s,
                            "Secret not found, provider will fail on use"
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

    #[tokio::test]
    async fn test_resolve_provider_secrets() {
        let dir = TempDir::new().unwrap();
        let vault_path = dir.path().join("test.vault");
        let mut vault = SecretVault::open(&vault_path, "master").unwrap();

        vault
            .set("anthropic_key", "sk-ant-real-key", EntryMetadata::default())
            .unwrap();

        let mut config = Config::default();
        let mut provider = ProviderConfig::test_config("claude-sonnet");
        provider.api_key = None;
        provider.secret_name = Some("anthropic_key".into());
        config.providers.insert("claude".into(), provider);

        // SecretVault implements AsyncSecretResolver
        resolve_provider_secrets(&mut config, &vault).await.unwrap();

        assert_eq!(
            config.providers.get("claude").unwrap().api_key.as_deref(),
            Some("sk-ant-real-key")
        );
    }
}
