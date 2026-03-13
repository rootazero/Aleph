//! Encrypted secret vault
//!
//! File-based encrypted storage for secrets using AES-256-GCM.
//! Location: ~/.aleph/secrets.vault
//!
//! This is a pure storage container — encryption/decryption is the caller's
//! responsibility (handled by SharedTokenManager).

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use tracing::{debug, info};

use super::types::{EncryptedEntry, EntryMetadata, SecretError, VaultData};

/// Current vault format version.
const VAULT_VERSION: u32 = 1;

/// Encrypted secret vault backed by a file.
///
/// A pure encrypted-entry storage container. Does not perform encryption
/// or decryption — callers are responsible for providing pre-encrypted
/// entries and decrypting retrieved entries.
pub struct SecretVault {
    data: VaultData,
    path: PathBuf,
}

impl SecretVault {
    /// Open or create a vault at the given path.
    pub fn open(path: impl Into<PathBuf>) -> Result<Self, SecretError> {
        let path = path.into();

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

        Ok(Self { data, path })
    }

    /// Create an empty vault (for when open() fails).
    pub fn empty(path: impl Into<PathBuf>) -> Self {
        Self {
            data: VaultData {
                version: VAULT_VERSION,
                entries: HashMap::new(),
            },
            path: path.into(),
        }
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

    /// Get a raw encrypted entry by name.
    pub fn get(&self, name: &str) -> Result<&EncryptedEntry, SecretError> {
        self.data
            .entries
            .get(name)
            .ok_or_else(|| SecretError::NotFound(name.to_string()))
    }

    /// Store a pre-encrypted entry. Preserves `created_at` if overwriting.
    pub fn set(&mut self, name: &str, mut entry: EncryptedEntry) -> Result<(), SecretError> {
        // Preserve created_at from existing entry if overwriting
        if let Some(existing) = self.data.entries.get(name) {
            entry.created_at = existing.created_at;
        }

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

    /// List all secret names with their metadata.
    pub fn list(&self) -> Vec<(String, &EntryMetadata)> {
        self.data
            .entries
            .iter()
            .map(|(name, entry)| (name.clone(), &entry.metadata))
            .collect()
    }

    /// List all entry names.
    pub fn list_names(&self) -> Vec<String> {
        self.data.entries.keys().cloned().collect()
    }

    /// Get all entries (for re-encryption during token reset).
    pub fn entries(&self) -> &HashMap<String, EncryptedEntry> {
        &self.data.entries
    }

    /// Replace all entries atomically (for re-encryption).
    pub fn replace_all(
        &mut self,
        entries: HashMap<String, EncryptedEntry>,
    ) -> Result<(), SecretError> {
        self.data.entries = entries;
        self.save()
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

#[cfg(test)]
mod tests {
    use super::*;
    use super::super::crypto::SecretsCrypto;
    use tempfile::TempDir;

    /// Helper: create a test vault in a temp directory.
    fn test_vault(dir: &TempDir) -> SecretVault {
        let path = dir.path().join("test.vault");
        SecretVault::open(path).unwrap()
    }

    /// Helper: encrypt a value and build an EncryptedEntry.
    fn make_entry(crypto: &SecretsCrypto, value: &str) -> EncryptedEntry {
        let encrypted = crypto.encrypt(value).unwrap();
        let now = chrono::Utc::now().timestamp();
        EncryptedEntry {
            ciphertext: encrypted.ciphertext,
            nonce: encrypted.nonce,
            salt: encrypted.salt,
            created_at: now,
            updated_at: now,
            metadata: EntryMetadata::default(),
        }
    }

    /// Helper: encrypt a value with metadata.
    fn make_entry_with_metadata(
        crypto: &SecretsCrypto,
        value: &str,
        metadata: EntryMetadata,
    ) -> EncryptedEntry {
        let encrypted = crypto.encrypt(value).unwrap();
        let now = chrono::Utc::now().timestamp();
        EncryptedEntry {
            ciphertext: encrypted.ciphertext,
            nonce: encrypted.nonce,
            salt: encrypted.salt,
            created_at: now,
            updated_at: now,
            metadata,
        }
    }

    #[test]
    fn test_set_and_get() {
        let dir = TempDir::new().unwrap();
        let mut vault = test_vault(&dir);
        let crypto = SecretsCrypto::new("test-master-key");

        let entry = make_entry(&crypto, "sk-ant-secret");
        vault.set("my_key", entry).unwrap();

        let retrieved = vault.get("my_key").unwrap();
        let decrypted = crypto
            .decrypt(&retrieved.ciphertext, &retrieved.nonce, &retrieved.salt)
            .unwrap();
        assert_eq!(decrypted, "sk-ant-secret");
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
        let crypto = SecretsCrypto::new("test-master-key");

        vault.set("key1", make_entry(&crypto, "value1")).unwrap();
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
        let crypto = SecretsCrypto::new("test-master-key");

        vault
            .set(
                "key1",
                make_entry_with_metadata(
                    &crypto,
                    "v1",
                    EntryMetadata {
                        provider: Some("anthropic".into()),
                        ..Default::default()
                    },
                ),
            )
            .unwrap();
        vault.set("key2", make_entry(&crypto, "v2")).unwrap();

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
        let crypto = SecretsCrypto::new("master");

        // Write
        {
            let mut vault = SecretVault::open(&vault_path).unwrap();
            vault
                .set("persistent_key", make_entry(&crypto, "persistent_value"))
                .unwrap();
        }

        // Read back
        {
            let vault = SecretVault::open(&vault_path).unwrap();
            let entry = vault.get("persistent_key").unwrap();
            let decrypted = crypto
                .decrypt(&entry.ciphertext, &entry.nonce, &entry.salt)
                .unwrap();
            assert_eq!(decrypted, "persistent_value");
        }
    }

    #[test]
    fn test_overwrite_preserves_created_at() {
        let dir = TempDir::new().unwrap();
        let mut vault = test_vault(&dir);
        let crypto = SecretsCrypto::new("test-master-key");

        vault.set("key", make_entry(&crypto, "v1")).unwrap();
        let created = vault.data.entries.get("key").unwrap().created_at;

        // Overwrite
        vault.set("key", make_entry(&crypto, "v2")).unwrap();
        let new_created = vault.data.entries.get("key").unwrap().created_at;
        let updated = vault.data.entries.get("key").unwrap().updated_at;

        assert_eq!(created, new_created); // created_at preserved
        assert!(updated >= created);
    }

    #[test]
    fn test_len_and_is_empty() {
        let dir = TempDir::new().unwrap();
        let mut vault = test_vault(&dir);
        let crypto = SecretsCrypto::new("test-master-key");

        assert!(vault.is_empty());
        assert_eq!(vault.len(), 0);

        vault.set("key", make_entry(&crypto, "val")).unwrap();
        assert!(!vault.is_empty());
        assert_eq!(vault.len(), 1);
    }

    #[test]
    fn test_metadata_stored() {
        let dir = TempDir::new().unwrap();
        let mut vault = test_vault(&dir);
        let crypto = SecretsCrypto::new("test-master-key");

        vault
            .set(
                "key",
                make_entry_with_metadata(
                    &crypto,
                    "val",
                    EntryMetadata {
                        description: Some("My Anthropic key".into()),
                        provider: Some("anthropic".into()),
                    },
                ),
            )
            .unwrap();

        let list = vault.list();
        let (_, meta) = list.iter().find(|(n, _)| n == "key").unwrap();
        assert_eq!(meta.description.as_deref(), Some("My Anthropic key"));
        assert_eq!(meta.provider.as_deref(), Some("anthropic"));
    }

    #[test]
    fn test_empty_vault() {
        let vault = SecretVault::empty("/tmp/nonexistent.vault");
        assert!(vault.is_empty());
        assert_eq!(vault.len(), 0);
    }

    #[test]
    fn test_list_names() {
        let dir = TempDir::new().unwrap();
        let mut vault = test_vault(&dir);
        let crypto = SecretsCrypto::new("test-master-key");

        vault.set("alpha", make_entry(&crypto, "a")).unwrap();
        vault.set("beta", make_entry(&crypto, "b")).unwrap();

        let mut names = vault.list_names();
        names.sort();
        assert_eq!(names, vec!["alpha", "beta"]);
    }

    #[test]
    fn test_entries() {
        let dir = TempDir::new().unwrap();
        let mut vault = test_vault(&dir);
        let crypto = SecretsCrypto::new("test-master-key");

        vault.set("k1", make_entry(&crypto, "v1")).unwrap();
        vault.set("k2", make_entry(&crypto, "v2")).unwrap();

        let entries = vault.entries();
        assert_eq!(entries.len(), 2);
        assert!(entries.contains_key("k1"));
        assert!(entries.contains_key("k2"));
    }

    #[test]
    fn test_replace_all() {
        let dir = TempDir::new().unwrap();
        let mut vault = test_vault(&dir);
        let crypto = SecretsCrypto::new("test-master-key");

        vault.set("old_key", make_entry(&crypto, "old")).unwrap();

        let mut new_entries = HashMap::new();
        new_entries.insert("new_key".to_string(), make_entry(&crypto, "new"));
        vault.replace_all(new_entries).unwrap();

        assert!(!vault.exists("old_key"));
        assert!(vault.exists("new_key"));
        assert_eq!(vault.len(), 1);
    }
}
