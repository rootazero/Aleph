//! Encrypted secret vault
//!
//! File-based encrypted storage for secrets using AES-256-GCM.
//! Location: ~/.aleph/secrets.vault
//!
//! This is a pure storage container — encryption/decryption is the caller's
//! responsibility (handled by `SharedTokenManager`).

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use tracing::{debug, info};

use super::types::{EncryptedEntry, SecretError, VaultData};
use crate::utils::vault_io::VaultIo;

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
        let io = Self::io_for(&path);

        let data = match io.read()? {
            Some(bytes) => {
                debug!(path = %path.display(), "Loading existing vault");
                let data: VaultData = bincode::deserialize(&bytes).map_err(|e| {
                    SecretError::Serialization(format!("Failed to deserialize vault: {e}"))
                })?;
                if data.version > VAULT_VERSION {
                    return Err(SecretError::Serialization(format!(
                        "Vault version {} is newer than supported version {}. Please upgrade Aleph.",
                        data.version, VAULT_VERSION
                    )));
                }
                data
            }
            None => {
                debug!(path = %path.display(), "Creating new vault");
                VaultData {
                    version: VAULT_VERSION,
                    entries: HashMap::new(),
                }
            }
        };

        Ok(Self { data, path })
    }

    /// Build a `VaultIo` for the configured vault path.
    fn io_for(path: &Path) -> VaultIo {
        VaultIo::new_with_path(path.to_path_buf())
    }

    /// Open the vault, or recover gracefully if an existing file is unreadable.
    ///
    /// `open()` returns `Err` *only* when a vault file is present but cannot be
    /// loaded — corruption, an incompatible future version, or an I/O error. A
    /// missing file is not an error; it yields a fresh empty vault. Because of
    /// that, the tempting `open(path).unwrap_or_else(|_| empty(path))` shortcut
    /// is a data-loss trap: it routes the present-but-unreadable case straight
    /// into an empty in-memory vault, and the next `save()` atomically
    /// overwrites the original file, destroying every stored secret.
    ///
    /// This helper instead renames the unreadable file aside to
    /// `<path>.corrupt-<unix_ts>` before returning a fresh vault, so the
    /// original bytes are preserved for recovery and the daemon still starts
    /// (graceful degradation per P7). If the rename itself fails the directory
    /// is almost certainly not writable, so the subsequent `save()` would fail
    /// too and no silent overwrite occurs.
    pub fn open_or_backup(path: impl Into<PathBuf>) -> Self {
        let path = path.into();
        match Self::open(&path) {
            Ok(vault) => vault,
            Err(open_err) => {
                if path.exists() {
                    let mut backup = path.clone().into_os_string();
                    backup.push(format!(".corrupt-{}", chrono::Utc::now().timestamp()));
                    let backup = PathBuf::from(backup);
                    match std::fs::rename(&path, &backup) {
                        Ok(()) => tracing::error!(
                            path = %path.display(),
                            backup = %backup.display(),
                            error = %open_err,
                            "Vault file could not be loaded; moved aside to preserve it. \
                             Starting with an empty vault."
                        ),
                        Err(rename_err) => tracing::error!(
                            path = %path.display(),
                            error = %open_err,
                            rename_error = %rename_err,
                            "Vault file could not be loaded and could not be backed up; \
                             the directory is likely not writable so secrets are not at risk \
                             of being overwritten."
                        ),
                    }
                }
                Self::empty(path)
            }
        }
    }

    /// Create an empty vault (for when `open()` fails).
    fn empty(path: impl Into<PathBuf>) -> Self {
        Self {
            data: VaultData {
                version: VAULT_VERSION,
                entries: HashMap::new(),
            },
            path: path.into(),
        }
    }

    /// Save vault to disk with atomic write + fcntl lock via `VaultIo`.
    fn save(&self) -> Result<(), SecretError> {
        let bytes = bincode::serialize(&self.data)
            .map_err(|e| SecretError::Serialization(format!("Failed to serialize vault: {e}")))?;

        // Ensure parent directory exists
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent)?;
        }

        // Atomic temp+fsync+rename serialised via fs2 fcntl lock on
        // `secrets.vault.lock`. Defense-in-depth even if the singleton lock
        // is bypassed.
        let io = Self::io_for(&self.path);
        io.write(&bytes)?;

        // Restrict vault file permissions on Unix (owner-only read/write).
        // tempfile creates files with 0o600 by default; this is defense-in-depth.
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let perms = std::fs::Permissions::from_mode(0o600);
            if let Err(e) = std::fs::set_permissions(&self.path, perms) {
                // Don't fail the entire save if chmod fails (e.g. on filesystems
                // that don't support Unix permissions). The tempfile-based
                // atomic write already created the file with 0o600 on Unix.
                tracing::warn!(
                    path = %self.path.display(),
                    error = %e,
                    "Failed to set vault file permissions — tempfile default 0o600 likely still applies"
                );
            }
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
        let now = chrono::Utc::now().timestamp();

        // Preserve created_at from existing entry if overwriting
        if let Some(existing) = self.data.entries.get(name) {
            entry.created_at = existing.created_at;
        }
        entry.updated_at = now;

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
    #[must_use]
    pub fn exists(&self, name: &str) -> bool {
        self.data.entries.contains_key(name)
    }

    /// List all entry names.
    #[must_use]
    pub fn list_names(&self) -> Vec<String> {
        self.data.entries.keys().cloned().collect()
    }

    /// Get all entries (for re-encryption during token reset).
    #[must_use]
    pub const fn entries(&self) -> &HashMap<String, EncryptedEntry> {
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

    /// Get the number of entries.
    #[must_use]
    pub fn len(&self) -> usize {
        self.data.entries.len()
    }

    /// Whether the vault holds no entries.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.data.entries.is_empty()
    }

    /// Get the default vault path.
    ///
    /// Falls back to `secrets.vault` in the current working directory only
    /// when the platform config directory cannot be determined (extremely
    /// rare). Callers that need a guaranteed absolute path should verify
    /// the result.
    #[must_use]
    pub fn default_path() -> PathBuf {
        crate::utils::paths::get_config_dir().map_or_else(|e| {
                tracing::warn!(
                    error = %e,
                    fallback = "secrets.vault",
                    "Failed to determine config directory; vault will be created in the current working directory. \
                     This is a fallback path — review permissions and ensure backups."
                );
                PathBuf::from("secrets.vault")
            }, |d| d.join("secrets.vault"))
    }
}

#[cfg(test)]
mod tests {
    use super::super::crypto::SecretsCrypto;
    use super::super::types::EntryMetadata;
    use super::*;
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
    fn test_len() {
        let dir = TempDir::new().unwrap();
        let mut vault = test_vault(&dir);
        let crypto = SecretsCrypto::new("test-master-key");

        assert_eq!(vault.len(), 0);

        vault.set("key", make_entry(&crypto, "val")).unwrap();
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

        let entry = vault.get("key").unwrap();
        let meta = &entry.metadata;
        assert_eq!(meta.description.as_deref(), Some("My Anthropic key"));
        assert_eq!(meta.provider.as_deref(), Some("anthropic"));
    }

    #[test]
    fn test_empty_vault() {
        let vault = SecretVault::empty("/tmp/nonexistent.vault");
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

    #[test]
    fn test_open_or_backup_preserves_unreadable_vault() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("data.vault");

        // Simulate an existing vault that `open()` rejects (future version).
        let future_data = VaultData {
            version: VAULT_VERSION + 1,
            entries: HashMap::new(),
        };
        std::fs::write(&path, bincode::serialize(&future_data).unwrap()).unwrap();
        let original_bytes = std::fs::read(&path).unwrap();

        // The dangerous `open().unwrap_or_else(empty)` would discard this and
        // overwrite it on the next save. `open_or_backup` must instead move it
        // aside and start fresh, leaving the original bytes recoverable.
        let mut vault = SecretVault::open_or_backup(&path);
        assert_eq!(vault.len(), 0);

        // A write after recovery must not have destroyed the original data.
        let crypto = SecretsCrypto::new("k");
        vault.set("new", make_entry(&crypto, "v")).unwrap();

        let backups: Vec<_> = std::fs::read_dir(dir.path())
            .unwrap()
            .filter_map(Result::ok)
            .filter(|e| {
                e.file_name()
                    .to_string_lossy()
                    .contains("data.vault.corrupt-")
            })
            .collect();
        assert_eq!(backups.len(), 1, "the unreadable vault should be backed up");
        assert_eq!(
            std::fs::read(backups[0].path()).unwrap(),
            original_bytes,
            "backed-up bytes must match the original unreadable vault"
        );
    }

    #[test]
    fn test_open_or_backup_no_file_is_fresh_vault() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("absent.vault");
        // A missing file is not an error path — no backup, just a fresh vault.
        let vault = SecretVault::open_or_backup(&path);
        assert_eq!(vault.len(), 0);
        assert!(!path.exists());
    }

    #[test]
    fn test_version_check_rejects_future_version() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("future.vault");

        // Write a vault with a future version
        let future_data = VaultData {
            version: VAULT_VERSION + 1,
            entries: HashMap::new(),
        };
        let bytes = bincode::serialize(&future_data).unwrap();
        std::fs::write(&path, bytes).unwrap();

        let result = SecretVault::open(&path);
        assert!(matches!(result, Err(SecretError::Serialization(_))));
    }
}
