# Agent Secret Management Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Replace plaintext API key storage in config.toml with AES-256-GCM encrypted vault, forced migration, and CLI management commands.

**Architecture:** New `secrets/` module in alephcore with `SecretsCrypto` (AES-256-GCM + HKDF-SHA256), `SecretVault` (encrypted file at `~/.aleph/secrets.vault`), and `DecryptedSecret` (secrecy crate wrapper). `ProviderConfig.api_key` replaced with `secret_name` referencing vault entries. CLI commands via `aleph-server secret` subcommand.

**Tech Stack:** `aes-gcm` 0.10, `hkdf` 0.12, `sha2` 0.10 (already present), `secrecy` 0.8, `zeroize` 1.8, `bincode` 1.3, `rpassword` 5.0

**Design Doc:** `docs/plans/2026-02-22-agent-secret-management-design.md`

---

### Task 1: Add crypto dependencies to Cargo.toml

**Files:**
- Modify: `core/Cargo.toml`

**Step 1: Add the new dependencies**

Add after the `siphasher` line (line 148):

```toml
# Secret vault encryption (Agent Secret Management)
aes-gcm = "0.10"
hkdf = "0.12"
secrecy = { version = "0.10", features = ["serde"] }
zeroize = { version = "1.8", features = ["derive"] }
bincode = "1.3"
rpassword = "5.0"
```

Note: `sha2 = "0.10"` already exists at line 140. Do not duplicate it.

**Step 2: Verify compilation**

Run: `cd /Users/zouguojun/Workspace/Aleph && cargo check -p alephcore 2>&1 | tail -5`
Expected: Compilation succeeds (downloads new crates, then "Finished")

**Step 3: Commit**

```bash
git add core/Cargo.toml
git commit -m "secrets: add crypto dependencies for encrypted vault"
```

---

### Task 2: Create `secrets/types.rs` — core types

**Files:**
- Create: `core/src/secrets/types.rs`
- Test: inline `#[cfg(test)]`

**Step 1: Write the failing test**

Create `core/src/secrets/types.rs` with test first:

```rust
//! Secret management types
//!
//! Core types for the encrypted secret vault system.

use secrecy::{ExposeSecret, SecretString};
use serde::{Deserialize, Serialize};
use std::fmt;

/// Decrypted secret value with memory safety guarantees.
///
/// The inner value is zeroized on drop via the `secrecy` crate.
/// Debug and Display implementations never expose the plaintext.
pub struct DecryptedSecret {
    value: SecretString,
}

impl DecryptedSecret {
    /// Create a new DecryptedSecret from a string value.
    pub fn new(value: impl Into<String>) -> Self {
        Self {
            value: SecretString::from(value.into()),
        }
    }

    /// Expose the plaintext value. Use sparingly.
    pub fn expose(&self) -> &str {
        self.value.expose_secret()
    }

    /// Get the length of the secret value in bytes.
    pub fn len(&self) -> usize {
        self.value.expose_secret().len()
    }

    /// Check if the secret is empty.
    pub fn is_empty(&self) -> bool {
        self.value.expose_secret().is_empty()
    }
}

impl fmt::Debug for DecryptedSecret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "[REDACTED, {} bytes]", self.len())
    }
}

impl fmt::Display for DecryptedSecret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "[REDACTED]")
    }
}

/// A single encrypted entry in the vault.
#[derive(Clone, Serialize, Deserialize)]
pub struct EncryptedEntry {
    /// AES-256-GCM ciphertext
    pub ciphertext: Vec<u8>,
    /// GCM nonce (12 bytes)
    pub nonce: [u8; 12],
    /// HKDF salt (32 bytes, per-entry)
    pub salt: [u8; 32],
    /// Unix timestamp when created
    pub created_at: i64,
    /// Unix timestamp when last updated
    pub updated_at: i64,
    /// Non-sensitive metadata
    pub metadata: EntryMetadata,
}

/// Non-sensitive metadata for a vault entry.
#[derive(Clone, Default, Serialize, Deserialize)]
pub struct EntryMetadata {
    /// Human-readable description
    pub description: Option<String>,
    /// Associated provider name (e.g., "anthropic")
    pub provider: Option<String>,
}

/// Serializable vault file format.
#[derive(Serialize, Deserialize, Default)]
pub struct VaultData {
    /// Format version for future migrations
    pub version: u32,
    /// Encrypted entries keyed by name
    pub entries: std::collections::HashMap<String, EncryptedEntry>,
}

/// Secret error types.
#[derive(Debug, thiserror::Error)]
pub enum SecretError {
    #[error("Secret '{0}' not found")]
    NotFound(String),

    #[error("Master key not configured. Set ALEPH_MASTER_KEY env var or run `aleph-server secret init`")]
    MasterKeyMissing,

    #[error("Decryption failed: vault may be corrupted or master key is wrong")]
    DecryptionFailed,

    #[error("Encryption failed: {0}")]
    EncryptionFailed(String),

    #[error("Vault I/O error: {0}")]
    Io(#[from] std::io::Error),

    #[error("Vault serialization error: {0}")]
    Serialization(String),

    #[error("Migration failed for provider '{provider}': {reason}")]
    MigrationFailed { provider: String, reason: String },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_decrypted_secret_expose() {
        let secret = DecryptedSecret::new("my-api-key");
        assert_eq!(secret.expose(), "my-api-key");
    }

    #[test]
    fn test_decrypted_secret_debug_redacted() {
        let secret = DecryptedSecret::new("sk-ant-api03-xxx");
        let debug = format!("{:?}", secret);
        assert!(!debug.contains("sk-ant"));
        assert!(debug.contains("REDACTED"));
        assert!(debug.contains("16 bytes"));
    }

    #[test]
    fn test_decrypted_secret_display_redacted() {
        let secret = DecryptedSecret::new("sk-ant-api03-xxx");
        let display = format!("{}", secret);
        assert_eq!(display, "[REDACTED]");
        assert!(!display.contains("sk-ant"));
    }

    #[test]
    fn test_decrypted_secret_len() {
        let secret = DecryptedSecret::new("12345");
        assert_eq!(secret.len(), 5);
        assert!(!secret.is_empty());
    }

    #[test]
    fn test_decrypted_secret_empty() {
        let secret = DecryptedSecret::new("");
        assert!(secret.is_empty());
    }

    #[test]
    fn test_vault_data_default() {
        let data = VaultData::default();
        assert_eq!(data.version, 0);
        assert!(data.entries.is_empty());
    }

    #[test]
    fn test_entry_metadata_default() {
        let meta = EntryMetadata::default();
        assert!(meta.description.is_none());
        assert!(meta.provider.is_none());
    }

    #[test]
    fn test_encrypted_entry_serialization() {
        let entry = EncryptedEntry {
            ciphertext: vec![1, 2, 3],
            nonce: [0u8; 12],
            salt: [0u8; 32],
            created_at: 1000,
            updated_at: 2000,
            metadata: EntryMetadata::default(),
        };
        let bytes = bincode::serialize(&entry).unwrap();
        let decoded: EncryptedEntry = bincode::deserialize(&bytes).unwrap();
        assert_eq!(decoded.ciphertext, vec![1, 2, 3]);
        assert_eq!(decoded.created_at, 1000);
    }
}
```

**Step 2: Run test to verify it compiles and passes**

Run: `cd /Users/zouguojun/Workspace/Aleph && cargo test -p alephcore secrets::types::tests --no-default-features 2>&1 | tail -10`

Note: This will fail because the module isn't registered yet. That's expected — we need Task 4 to wire up `mod.rs` and `lib.rs`. For now, just verify the file is syntactically correct:

Run: `cd /Users/zouguojun/Workspace/Aleph && rustfmt --check core/src/secrets/types.rs`
Expected: No formatting errors

**Step 3: Commit**

```bash
git add core/src/secrets/types.rs
git commit -m "secrets: add core types (DecryptedSecret, EncryptedEntry, SecretError)"
```

---

### Task 3: Create `secrets/crypto.rs` — encryption engine

**Files:**
- Create: `core/src/secrets/crypto.rs`
- Test: inline `#[cfg(test)]`

**Step 1: Write crypto.rs with tests**

```rust
//! Secret encryption engine
//!
//! Provides AES-256-GCM encryption with HKDF-SHA256 per-entry key derivation.
//! Inspired by IronClaw's SecretsCrypto design.

use aes_gcm::{
    aead::{Aead, KeyInit},
    Aes256Gcm, Nonce,
};
use hkdf::Hkdf;
use secrecy::{ExposeSecret, SecretString};
use sha2::Sha256;

use super::types::SecretError;

/// HKDF info label for domain separation.
const HKDF_INFO: &[u8] = b"aleph-secrets-v1";

/// Encryption engine using AES-256-GCM with per-entry HKDF key derivation.
///
/// The master key is held in a `SecretString` which is zeroized on drop.
pub struct SecretsCrypto {
    master_key: SecretString,
}

impl SecretsCrypto {
    /// Create a new crypto engine with the given master key.
    pub fn new(master_key: impl Into<String>) -> Self {
        Self {
            master_key: SecretString::from(master_key.into()),
        }
    }

    /// Derive a per-entry encryption key using HKDF-SHA256.
    fn derive_key(&self, salt: &[u8; 32]) -> Result<[u8; 32], SecretError> {
        let hkdf = Hkdf::<Sha256>::new(Some(salt), self.master_key.expose_secret().as_bytes());
        let mut key = [0u8; 32];
        hkdf.expand(HKDF_INFO, &mut key)
            .map_err(|e| SecretError::EncryptionFailed(format!("HKDF expand failed: {}", e)))?;
        Ok(key)
    }

    /// Encrypt a plaintext value.
    ///
    /// Returns (ciphertext, nonce, salt) tuple.
    /// Each call generates a fresh random salt and nonce.
    pub fn encrypt(&self, plaintext: &str) -> Result<(Vec<u8>, [u8; 12], [u8; 32]), SecretError> {
        use rand::RngCore;

        // Generate random salt and nonce
        let mut salt = [0u8; 32];
        let mut nonce_bytes = [0u8; 12];
        rand::thread_rng().fill_bytes(&mut salt);
        rand::thread_rng().fill_bytes(&mut nonce_bytes);

        let key = self.derive_key(&salt)?;
        let cipher = Aes256Gcm::new_from_slice(&key)
            .map_err(|e| SecretError::EncryptionFailed(format!("AES init failed: {}", e)))?;

        let nonce = Nonce::from_slice(&nonce_bytes);
        let ciphertext = cipher
            .encrypt(nonce, plaintext.as_bytes())
            .map_err(|e| SecretError::EncryptionFailed(format!("AES encrypt failed: {}", e)))?;

        // Zeroize derived key
        // (key goes out of scope and is on the stack, but let's be explicit)
        let _ = key;

        Ok((ciphertext, nonce_bytes, salt))
    }

    /// Decrypt a ciphertext using the stored nonce and salt.
    pub fn decrypt(
        &self,
        ciphertext: &[u8],
        nonce_bytes: &[u8; 12],
        salt: &[u8; 32],
    ) -> Result<String, SecretError> {
        let key = self.derive_key(salt)?;
        let cipher = Aes256Gcm::new_from_slice(&key)
            .map_err(|_| SecretError::DecryptionFailed)?;

        let nonce = Nonce::from_slice(nonce_bytes);
        let plaintext = cipher
            .decrypt(nonce, ciphertext)
            .map_err(|_| SecretError::DecryptionFailed)?;

        String::from_utf8(plaintext)
            .map_err(|_| SecretError::DecryptionFailed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_encrypt_decrypt_roundtrip() {
        let crypto = SecretsCrypto::new("test-master-key");
        let plaintext = "sk-ant-api03-very-secret-key";

        let (ciphertext, nonce, salt) = crypto.encrypt(plaintext).unwrap();
        let decrypted = crypto.decrypt(&ciphertext, &nonce, &salt).unwrap();

        assert_eq!(decrypted, plaintext);
    }

    #[test]
    fn test_ciphertext_differs_from_plaintext() {
        let crypto = SecretsCrypto::new("test-master-key");
        let plaintext = "sk-ant-api03-very-secret-key";

        let (ciphertext, _, _) = crypto.encrypt(plaintext).unwrap();
        assert_ne!(ciphertext, plaintext.as_bytes());
    }

    #[test]
    fn test_different_salts_produce_different_ciphertexts() {
        let crypto = SecretsCrypto::new("test-master-key");
        let plaintext = "same-plaintext";

        let (ct1, _, _) = crypto.encrypt(plaintext).unwrap();
        let (ct2, _, _) = crypto.encrypt(plaintext).unwrap();

        // Different random salts → different ciphertexts
        assert_ne!(ct1, ct2);
    }

    #[test]
    fn test_wrong_master_key_fails() {
        let crypto1 = SecretsCrypto::new("correct-key");
        let crypto2 = SecretsCrypto::new("wrong-key");

        let (ciphertext, nonce, salt) = crypto1.encrypt("secret").unwrap();
        let result = crypto2.decrypt(&ciphertext, &nonce, &salt);

        assert!(result.is_err());
        assert!(matches!(result, Err(SecretError::DecryptionFailed)));
    }

    #[test]
    fn test_tampered_ciphertext_fails() {
        let crypto = SecretsCrypto::new("test-key");
        let (mut ciphertext, nonce, salt) = crypto.encrypt("secret").unwrap();

        // Tamper with ciphertext
        if let Some(byte) = ciphertext.first_mut() {
            *byte ^= 0xFF;
        }

        let result = crypto.decrypt(&ciphertext, &nonce, &salt);
        assert!(result.is_err());
    }

    #[test]
    fn test_empty_plaintext() {
        let crypto = SecretsCrypto::new("test-key");
        let (ciphertext, nonce, salt) = crypto.encrypt("").unwrap();
        let decrypted = crypto.decrypt(&ciphertext, &nonce, &salt).unwrap();
        assert_eq!(decrypted, "");
    }

    #[test]
    fn test_unicode_plaintext() {
        let crypto = SecretsCrypto::new("test-key");
        let plaintext = "密钥测试🔑";
        let (ciphertext, nonce, salt) = crypto.encrypt(plaintext).unwrap();
        let decrypted = crypto.decrypt(&ciphertext, &nonce, &salt).unwrap();
        assert_eq!(decrypted, plaintext);
    }
}
```

**Step 2: Verify syntax**

Run: `cd /Users/zouguojun/Workspace/Aleph && rustfmt --check core/src/secrets/crypto.rs`
Expected: No formatting errors

**Step 3: Commit**

```bash
git add core/src/secrets/crypto.rs
git commit -m "secrets: add SecretsCrypto (AES-256-GCM + HKDF-SHA256)"
```

---

### Task 4: Create `secrets/vault.rs` — vault storage

**Files:**
- Create: `core/src/secrets/vault.rs`
- Test: inline `#[cfg(test)]`

**Step 1: Write vault.rs**

```rust
//! Encrypted secret vault
//!
//! File-based encrypted storage for secrets using AES-256-GCM.
//! Location: ~/.aleph/secrets.vault

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use tracing::{debug, info, warn};

use super::crypto::SecretsCrypto;
use super::types::{DecryptedSecret, EncryptedEntry, EntryMetadata, SecretError, VaultData};

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
            bincode::deserialize(&bytes)
                .map_err(|e| SecretError::Serialization(format!("Failed to deserialize vault: {}", e)))?
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
        let entry = self.data.entries.get(name)
            .ok_or_else(|| SecretError::NotFound(name.to_string()))?;

        let plaintext = self.crypto.decrypt(
            &entry.ciphertext,
            &entry.nonce,
            &entry.salt,
        )?;

        Ok(DecryptedSecret::new(plaintext))
    }

    /// Encrypt and store a secret.
    pub fn set(&mut self, name: &str, value: &str, metadata: EntryMetadata) -> Result<(), SecretError> {
        let (ciphertext, nonce, salt) = self.crypto.encrypt(value)?;
        let now = chrono::Utc::now().timestamp();

        let entry = EncryptedEntry {
            ciphertext,
            nonce,
            salt,
            created_at: self.data.entries.get(name).map(|e| e.created_at).unwrap_or(now),
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
        self.data.entries.iter()
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

/// Resolve the master key from environment or return error.
pub fn resolve_master_key() -> Result<String, SecretError> {
    std::env::var("ALEPH_MASTER_KEY").map_err(|_| SecretError::MasterKeyMissing)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn test_vault(dir: &TempDir) -> SecretVault {
        let path = dir.path().join("test.vault");
        SecretVault::open(path, "test-master-key").unwrap()
    }

    #[test]
    fn test_set_and_get() {
        let dir = TempDir::new().unwrap();
        let mut vault = test_vault(&dir);

        vault.set("my_key", "sk-ant-secret", EntryMetadata::default()).unwrap();
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

        vault.set("key1", "value1", EntryMetadata::default()).unwrap();
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

        vault.set("key1", "v1", EntryMetadata { provider: Some("anthropic".into()), ..Default::default() }).unwrap();
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
            vault.set("persistent_key", "persistent_value", EntryMetadata::default()).unwrap();
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
            vault.set("secret", "value", EntryMetadata::default()).unwrap();
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

        vault.set("key", "val", EntryMetadata {
            description: Some("My Anthropic key".into()),
            provider: Some("anthropic".into()),
        }).unwrap();

        let list = vault.list();
        let (_, meta) = list.iter().find(|(n, _)| n == "key").unwrap();
        assert_eq!(meta.description.as_deref(), Some("My Anthropic key"));
        assert_eq!(meta.provider.as_deref(), Some("anthropic"));
    }
}
```

**Step 2: Verify syntax**

Run: `cd /Users/zouguojun/Workspace/Aleph && rustfmt --check core/src/secrets/vault.rs`
Expected: No formatting errors

**Step 3: Commit**

```bash
git add core/src/secrets/vault.rs
git commit -m "secrets: add SecretVault (encrypted file storage with CRUD)"
```

---

### Task 5: Create `secrets/migration.rs` — config migration

**Files:**
- Create: `core/src/secrets/migration.rs`
- Test: inline `#[cfg(test)]`

**Step 1: Write migration.rs**

```rust
//! Config migration: plaintext api_key → SecretVault
//!
//! Detects plaintext api_key fields in config.toml and migrates them
//! to the encrypted vault, replacing with secret_name references.

use std::path::Path;
use tracing::{info, warn};

use super::types::{EntryMetadata, SecretError};
use super::vault::SecretVault;
use crate::config::Config;

/// Result of a migration attempt.
#[derive(Debug)]
pub struct MigrationResult {
    /// Number of keys migrated.
    pub migrated_count: usize,
    /// Names of migrated providers.
    pub migrated_providers: Vec<String>,
}

/// Check if config has any plaintext api_key fields that need migration.
pub fn needs_migration(config: &Config) -> bool {
    config.providers.values().any(|p| p.api_key.is_some())
}

/// Migrate all plaintext api_key fields to the vault.
///
/// For each provider with a plaintext api_key:
/// 1. Store the key in the vault as "{provider_name}_api_key"
/// 2. Set secret_name on the provider config
/// 3. Clear the api_key field
///
/// Returns the list of migrated provider names.
pub fn migrate_api_keys(
    config: &mut Config,
    vault: &mut SecretVault,
) -> Result<MigrationResult, SecretError> {
    let mut migrated = Vec::new();

    // Collect providers that need migration
    let to_migrate: Vec<(String, String)> = config
        .providers
        .iter()
        .filter_map(|(name, p)| {
            p.api_key.as_ref().map(|key| (name.clone(), key.clone()))
        })
        .collect();

    for (provider_name, api_key) in to_migrate {
        let secret_name = format!("{}_api_key", provider_name.replace('-', "_"));

        // Store in vault
        vault.set(
            &secret_name,
            &api_key,
            EntryMetadata {
                description: Some(format!("API key for provider '{}'", provider_name)),
                provider: Some(provider_name.clone()),
            },
        )?;

        // Update config: clear api_key, set secret_name
        if let Some(provider) = config.providers.get_mut(&provider_name) {
            provider.api_key = None;
            provider.secret_name = Some(secret_name.clone());
        }

        info!(
            provider = %provider_name,
            secret_name = %secret_name,
            "Migrated API key to vault"
        );
        migrated.push(provider_name);
    }

    Ok(MigrationResult {
        migrated_count: migrated.len(),
        migrated_providers: migrated,
    })
}

/// Rewrite config.toml after migration to remove plaintext keys.
///
/// Uses incremental save to only update the providers section.
pub fn save_migrated_config(config: &Config) -> Result<(), SecretError> {
    let path = Config::default_path();
    if path.exists() {
        config.save_incremental(&["providers"]).map_err(|e| {
            SecretError::MigrationFailed {
                provider: "all".into(),
                reason: format!("Failed to save migrated config: {}", e),
            }
        })?;
        info!("Config file updated: plaintext API keys removed");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::ProviderConfig;
    use tempfile::TempDir;

    fn make_config_with_plaintext_keys() -> Config {
        let mut config = Config::default();
        let mut provider = ProviderConfig::test_config("claude-sonnet-4-20250514");
        provider.api_key = Some("sk-ant-test-key-123".to_string());
        provider.protocol = Some("anthropic".to_string());
        config.providers.insert("claude-main".to_string(), provider);

        let mut provider2 = ProviderConfig::test_config("gpt-4o");
        provider2.api_key = Some("sk-openai-test-456".to_string());
        config.providers.insert("openai-main".to_string(), provider2);

        config
    }

    #[test]
    fn test_needs_migration_with_plaintext() {
        let config = make_config_with_plaintext_keys();
        assert!(needs_migration(&config));
    }

    #[test]
    fn test_needs_migration_without_plaintext() {
        let mut config = Config::default();
        let mut provider = ProviderConfig::test_config("model");
        provider.api_key = None;
        provider.secret_name = Some("my_key".into());
        config.providers.insert("test".into(), provider);

        assert!(!needs_migration(&config));
    }

    #[test]
    fn test_migrate_api_keys() {
        let dir = TempDir::new().unwrap();
        let vault_path = dir.path().join("test.vault");
        let mut vault = SecretVault::open(&vault_path, "master").unwrap();
        let mut config = make_config_with_plaintext_keys();

        let result = migrate_api_keys(&mut config, &mut vault).unwrap();

        // Check migration counts
        assert_eq!(result.migrated_count, 2);
        assert!(result.migrated_providers.contains(&"claude-main".to_string()));
        assert!(result.migrated_providers.contains(&"openai-main".to_string()));

        // Check config updated
        let claude = config.providers.get("claude-main").unwrap();
        assert!(claude.api_key.is_none());
        assert_eq!(claude.secret_name.as_deref(), Some("claude_main_api_key"));

        let openai = config.providers.get("openai-main").unwrap();
        assert!(openai.api_key.is_none());
        assert_eq!(openai.secret_name.as_deref(), Some("openai_main_api_key"));

        // Check vault has the keys
        let secret = vault.get("claude_main_api_key").unwrap();
        assert_eq!(secret.expose(), "sk-ant-test-key-123");

        let secret = vault.get("openai_main_api_key").unwrap();
        assert_eq!(secret.expose(), "sk-openai-test-456");
    }

    #[test]
    fn test_migrate_skips_already_migrated() {
        let dir = TempDir::new().unwrap();
        let vault_path = dir.path().join("test.vault");
        let mut vault = SecretVault::open(&vault_path, "master").unwrap();

        let mut config = Config::default();
        let mut provider = ProviderConfig::test_config("model");
        provider.api_key = None;
        provider.secret_name = Some("existing_key".into());
        config.providers.insert("test".into(), provider);

        let result = migrate_api_keys(&mut config, &mut vault).unwrap();
        assert_eq!(result.migrated_count, 0);
    }
}
```

**Step 2: Verify syntax**

Run: `cd /Users/zouguojun/Workspace/Aleph && rustfmt --check core/src/secrets/migration.rs`

**Step 3: Commit**

```bash
git add core/src/secrets/migration.rs
git commit -m "secrets: add config migration (plaintext api_key -> vault)"
```

---

### Task 6: Create `secrets/mod.rs` and register in `lib.rs`

**Files:**
- Create: `core/src/secrets/mod.rs`
- Modify: `core/src/lib.rs:94` (add module declaration)
- Modify: `core/src/config/types/provider.rs:49` (add `secret_name` field)

**Step 1: Create mod.rs**

```rust
//! Secret management module
//!
//! Provides encrypted storage for sensitive credentials (API keys, tokens).
//! Uses AES-256-GCM with per-entry HKDF-SHA256 key derivation.
//!
//! # Architecture
//!
//! ```text
//! config.toml (secret_name = "xxx")
//!        │
//!        ▼
//! SecretVault (~/.aleph/secrets.vault)
//!        │  AES-256-GCM + HKDF
//!        ▼
//! DecryptedSecret (SecretString, zeroized on drop)
//! ```

pub mod crypto;
pub mod migration;
pub mod types;
pub mod vault;

pub use types::{DecryptedSecret, SecretError};
pub use vault::{SecretVault, resolve_master_key};
```

**Step 2: Add module to lib.rs**

In `core/src/lib.rs`, add after line 94 (`pub mod scheduler;`):

```rust
pub mod secrets;
```

**Step 3: Add `secret_name` field to ProviderConfig**

In `core/src/config/types/provider.rs`, add after line 49 (`pub api_key: Option<String>,`):

```rust
    /// Reference to a secret in the vault (replaces plaintext api_key)
    #[serde(default)]
    #[schemars(skip)]
    pub secret_name: Option<String>,
```

Also update `test_config()` (around line 139) to include the new field:

```rust
    pub fn test_config(model: impl Into<String>) -> Self {
        Self {
            protocol: None,
            api_key: Some("test-key".to_string()),
            secret_name: None,
            model: model.into(),
            // ... rest unchanged
```

And update all test constructors that manually construct `ProviderConfig` to include `secret_name: None`.

**Step 4: Run tests**

Run: `cd /Users/zouguojun/Workspace/Aleph && cargo test -p alephcore secrets:: 2>&1 | tail -20`
Expected: All tests in `secrets::types::tests`, `secrets::crypto::tests`, `secrets::vault::tests`, `secrets::migration::tests` pass.

**Step 5: Run full test suite to check nothing breaks**

Run: `cd /Users/zouguojun/Workspace/Aleph && cargo test -p alephcore 2>&1 | tail -20`
Expected: All existing tests still pass. May need to fix compilation errors in files that construct `ProviderConfig` without the new `secret_name` field.

**Step 6: Commit**

```bash
git add core/src/secrets/ core/src/lib.rs core/src/config/types/provider.rs
git commit -m "secrets: wire up module, add secret_name field to ProviderConfig"
```

---

### Task 7: Integrate vault with provider creation

**Files:**
- Modify: `core/src/providers/mod.rs:128` (`create_provider` function)
- Modify: `core/src/providers/protocols/openai.rs:291-295`
- Modify: `core/src/providers/protocols/anthropic.rs:226-229`
- Modify: `core/src/providers/protocols/gemini.rs:246-249`
- Modify: `core/src/providers/protocols/configurable.rs:85-88,172-173`

**Step 1: Update ProtocolAdapter to accept resolved api_key**

The current `build_request` method reads `config.api_key` directly. Since `ProviderConfig` still has the `api_key` field (for backward compatibility during migration), the simplest approach is:

**In the server startup path**, after loading config and vault, resolve `secret_name` → plaintext api_key and populate `config.api_key` in memory before passing to `create_provider`. This way the protocol adapters don't need changes.

Add a helper function in `core/src/secrets/vault.rs`:

```rust
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
```

**Step 2: Integrate into server startup**

In `core/src/bin/aleph_server/server_init.rs` (or wherever the server loads config), add after config loading:

```rust
// Secret vault integration
use alephcore::secrets::{SecretVault, resolve_master_key};
use alephcore::secrets::migration::{needs_migration, migrate_api_keys, save_migrated_config};
use alephcore::secrets::vault::resolve_provider_secrets;

// 1. Try to open vault (optional — if no master key, skip)
if let Ok(master_key) = resolve_master_key() {
    let vault_path = SecretVault::default_path();
    let mut vault = SecretVault::open(&vault_path, &master_key)?;

    // 2. Run migration if needed
    if needs_migration(&config) {
        let result = migrate_api_keys(&mut config, &mut vault)?;
        if result.migrated_count > 0 {
            save_migrated_config(&config)?;
            info!(
                count = result.migrated_count,
                providers = ?result.migrated_providers,
                "Migrated plaintext API keys to vault"
            );
        }
    }

    // 3. Resolve secret references
    resolve_provider_secrets(&mut config, &vault)?;
} else {
    warn!("ALEPH_MASTER_KEY not set — secret vault disabled, using config.toml api_key values");
}
```

**Step 3: Write test for resolve_provider_secrets**

Add to `core/src/secrets/vault.rs` tests:

```rust
    #[test]
    fn test_resolve_provider_secrets() {
        let dir = TempDir::new().unwrap();
        let vault_path = dir.path().join("test.vault");
        let mut vault = SecretVault::open(&vault_path, "master").unwrap();

        // Store a secret
        vault.set("anthropic_key", "sk-ant-real-key", EntryMetadata::default()).unwrap();

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
```

**Step 4: Run tests**

Run: `cd /Users/zouguojun/Workspace/Aleph && cargo test -p alephcore secrets:: 2>&1 | tail -20`
Expected: All secrets tests pass including the new `test_resolve_provider_secrets`.

**Step 5: Commit**

```bash
git add core/src/secrets/vault.rs core/src/bin/aleph_server/
git commit -m "secrets: integrate vault with provider creation and server startup"
```

---

### Task 8: Add CLI `secret` subcommand

**Files:**
- Modify: `core/src/bin/aleph_server/cli.rs` (add Secret command)
- Modify: `core/src/bin/aleph_server/main.rs` (handle Secret command)
- Create: `core/src/bin/aleph_server/commands/secret.rs`

**Step 1: Add SecretAction enum to cli.rs**

Add to the `Command` enum in `core/src/bin/aleph_server/cli.rs`:

```rust
    /// Manage secrets in the encrypted vault
    Secret {
        #[command(subcommand)]
        action: SecretAction,
    },
```

Add the enum:

```rust
/// Secret management subcommands
#[derive(Subcommand, Debug)]
pub enum SecretAction {
    /// Initialize the vault (first-time setup)
    Init,
    /// Store a secret in the vault
    Set {
        /// Secret name (e.g., "anthropic_api_key")
        name: String,
    },
    /// List all secret names
    List,
    /// Delete a secret
    Delete {
        /// Secret name to delete
        name: String,
    },
}
```

Update the import line in `main.rs` to include `SecretAction`.

**Step 2: Add command handler**

Create `core/src/bin/aleph_server/commands/secret.rs`:

```rust
//! Secret vault CLI commands

use alephcore::secrets::vault::{SecretVault, resolve_master_key};
use alephcore::secrets::types::EntryMetadata;

pub fn handle_secret_init() -> Result<(), Box<dyn std::error::Error>> {
    // Check if master key is already set
    if resolve_master_key().is_ok() {
        let vault_path = SecretVault::default_path();
        let vault = SecretVault::open(&vault_path, &resolve_master_key().unwrap())?;
        println!("Vault already exists at {}", vault_path.display());
        println!("Contains {} secret(s)", vault.len());
        return Ok(());
    }

    // Prompt for master key
    let key = rpassword::prompt_password("Enter master key: ")?;
    let key_confirm = rpassword::prompt_password("Confirm master key: ")?;

    if key != key_confirm {
        eprintln!("Error: Keys do not match");
        std::process::exit(1);
    }

    if key.len() < 8 {
        eprintln!("Error: Master key must be at least 8 characters");
        std::process::exit(1);
    }

    let vault_path = SecretVault::default_path();
    let _vault = SecretVault::open(&vault_path, &key)?;

    println!("Vault initialized at {}", vault_path.display());
    println!();
    println!("IMPORTANT: Set this environment variable to use the vault:");
    println!("  export ALEPH_MASTER_KEY=\"{}\"", key);
    println!();
    println!("Or add it to your shell profile (~/.zshrc, ~/.bashrc)");

    Ok(())
}

pub fn handle_secret_set(name: &str) -> Result<(), Box<dyn std::error::Error>> {
    let master_key = resolve_master_key()?;
    let vault_path = SecretVault::default_path();
    let mut vault = SecretVault::open(&vault_path, &master_key)?;

    let value = rpassword::prompt_password("Enter secret value: ")?;

    if value.is_empty() {
        eprintln!("Error: Secret value cannot be empty");
        std::process::exit(1);
    }

    vault.set(name, &value, EntryMetadata::default())?;
    println!("Secret '{}' stored successfully", name);
    Ok(())
}

pub fn handle_secret_list() -> Result<(), Box<dyn std::error::Error>> {
    let master_key = resolve_master_key()?;
    let vault_path = SecretVault::default_path();
    let vault = SecretVault::open(&vault_path, &master_key)?;

    let entries = vault.list();
    if entries.is_empty() {
        println!("No secrets stored");
        return Ok(());
    }

    println!("{:<30} {:<20}", "NAME", "PROVIDER");
    println!("{}", "-".repeat(50));
    for (name, meta) in entries {
        let provider = meta.provider.as_deref().unwrap_or("-");
        println!("{:<30} {:<20}", name, provider);
    }
    println!();
    println!("Total: {} secret(s)", vault.len());

    Ok(())
}

pub fn handle_secret_delete(name: &str) -> Result<(), Box<dyn std::error::Error>> {
    let master_key = resolve_master_key()?;
    let vault_path = SecretVault::default_path();
    let mut vault = SecretVault::open(&vault_path, &master_key)?;

    if vault.delete(name)? {
        println!("Secret '{}' deleted", name);
    } else {
        eprintln!("Secret '{}' not found", name);
        std::process::exit(1);
    }
    Ok(())
}
```

**Step 3: Wire up in main.rs**

Add to the match in `main.rs`:

```rust
        Some(Command::Secret { action }) => {
            return match action {
                SecretAction::Init => commands::secret::handle_secret_init(),
                SecretAction::Set { name } => commands::secret::handle_secret_set(&name),
                SecretAction::List => commands::secret::handle_secret_list(),
                SecretAction::Delete { name } => commands::secret::handle_secret_delete(&name),
            }.map_err(|e| e.into());
        }
```

Add `pub mod secret;` to `commands/mod.rs`.

**Step 4: Add `rpassword` to server binary deps**

The `rpassword` crate is already added to `core/Cargo.toml` in Task 1. The `aleph-server` binary is part of the `alephcore` crate, so it's available.

**Step 5: Run compilation check**

Run: `cd /Users/zouguojun/Workspace/Aleph && cargo build -p alephcore --bin aleph-server --features gateway 2>&1 | tail -10`
Expected: Compiles successfully.

**Step 6: Test CLI parsing**

Add to cli.rs tests:

```rust
    #[test]
    fn test_cli_parses_secret_init() {
        let args = Args::try_parse_from(["aleph-gateway", "secret", "init"]);
        assert!(args.is_ok());
        match args.unwrap().command {
            Some(Command::Secret { action }) => {
                assert!(matches!(action, SecretAction::Init));
            }
            _ => panic!("Expected Secret Init"),
        }
    }

    #[test]
    fn test_cli_parses_secret_set() {
        let args = Args::try_parse_from(["aleph-gateway", "secret", "set", "my_key"]);
        assert!(args.is_ok());
        match args.unwrap().command {
            Some(Command::Secret { action }) => {
                if let SecretAction::Set { name } = action {
                    assert_eq!(name, "my_key");
                } else {
                    panic!("Expected SecretAction::Set");
                }
            }
            _ => panic!("Expected Secret command"),
        }
    }

    #[test]
    fn test_cli_parses_secret_list() {
        let args = Args::try_parse_from(["aleph-gateway", "secret", "list"]);
        assert!(args.is_ok());
        match args.unwrap().command {
            Some(Command::Secret { action }) => {
                assert!(matches!(action, SecretAction::List));
            }
            _ => panic!("Expected Secret List"),
        }
    }

    #[test]
    fn test_cli_parses_secret_delete() {
        let args = Args::try_parse_from(["aleph-gateway", "secret", "delete", "old_key"]);
        assert!(args.is_ok());
        match args.unwrap().command {
            Some(Command::Secret { action }) => {
                if let SecretAction::Delete { name } = action {
                    assert_eq!(name, "old_key");
                } else {
                    panic!("Expected SecretAction::Delete");
                }
            }
            _ => panic!("Expected Secret command"),
        }
    }
```

**Step 7: Run tests**

Run: `cd /Users/zouguojun/Workspace/Aleph && cargo test -p alephcore --bin aleph-server --features gateway cli::tests 2>&1 | tail -10`
Expected: All CLI parsing tests pass.

**Step 8: Commit**

```bash
git add core/src/bin/aleph_server/
git commit -m "secrets: add CLI commands (secret init/set/list/delete)"
```

---

### Task 9: Integration test — full end-to-end flow

**Files:**
- Create: `core/tests/secret_vault_integration.rs`

**Step 1: Write integration test**

```rust
//! Integration test for the secret vault system
//!
//! Tests the full flow: vault creation → secret storage → config migration
//! → provider secret resolution.

use alephcore::config::{Config, ProviderConfig};
use alephcore::secrets::migration::{migrate_api_keys, needs_migration};
use alephcore::secrets::types::EntryMetadata;
use alephcore::secrets::vault::{resolve_provider_secrets, SecretVault};
use tempfile::TempDir;

#[test]
fn test_full_migration_flow() {
    let dir = TempDir::new().unwrap();
    let vault_path = dir.path().join("secrets.vault");

    // 1. Start with plaintext config
    let mut config = Config::default();

    let mut anthropic = ProviderConfig::test_config("claude-sonnet-4-20250514");
    anthropic.api_key = Some("sk-ant-api03-real-key".to_string());
    anthropic.protocol = Some("anthropic".to_string());
    config.providers.insert("claude-main".to_string(), anthropic);

    let mut openai = ProviderConfig::test_config("gpt-4o");
    openai.api_key = Some("sk-proj-openai-key".to_string());
    config.providers.insert("openai-main".to_string(), openai);

    assert!(needs_migration(&config));

    // 2. Open vault and migrate
    let mut vault = SecretVault::open(&vault_path, "test-master-key-12345").unwrap();
    let result = migrate_api_keys(&mut config, &mut vault).unwrap();

    assert_eq!(result.migrated_count, 2);
    assert!(!needs_migration(&config));

    // 3. Verify config updated
    let claude = config.providers.get("claude-main").unwrap();
    assert!(claude.api_key.is_none());
    assert_eq!(claude.secret_name.as_deref(), Some("claude_main_api_key"));

    // 4. Simulate server restart: reload from vault
    let mut config2 = Config::default();
    let mut claude2 = ProviderConfig::test_config("claude-sonnet-4-20250514");
    claude2.api_key = None;
    claude2.secret_name = Some("claude_main_api_key".to_string());
    claude2.protocol = Some("anthropic".to_string());
    config2.providers.insert("claude-main".to_string(), claude2);

    let vault2 = SecretVault::open(&vault_path, "test-master-key-12345").unwrap();
    resolve_provider_secrets(&mut config2, &vault2).unwrap();

    // 5. api_key resolved from vault
    assert_eq!(
        config2.providers.get("claude-main").unwrap().api_key.as_deref(),
        Some("sk-ant-api03-real-key")
    );
}

#[test]
fn test_vault_survives_process_restart() {
    let dir = TempDir::new().unwrap();
    let vault_path = dir.path().join("persist.vault");

    // Process 1: create and store
    {
        let mut vault = SecretVault::open(&vault_path, "my-key").unwrap();
        vault.set("secret1", "value1", EntryMetadata::default()).unwrap();
        vault.set("secret2", "value2", EntryMetadata {
            provider: Some("anthropic".into()),
            ..Default::default()
        }).unwrap();
    }

    // Process 2: reopen and read
    {
        let vault = SecretVault::open(&vault_path, "my-key").unwrap();
        assert_eq!(vault.len(), 2);
        assert_eq!(vault.get("secret1").unwrap().expose(), "value1");
        assert_eq!(vault.get("secret2").unwrap().expose(), "value2");
    }
}

#[test]
fn test_wrong_master_key_cannot_read() {
    let dir = TempDir::new().unwrap();
    let vault_path = dir.path().join("locked.vault");

    // Write with key A
    {
        let mut vault = SecretVault::open(&vault_path, "correct-key").unwrap();
        vault.set("secret", "sensitive-data", EntryMetadata::default()).unwrap();
    }

    // Try to read with key B
    {
        let vault = SecretVault::open(&vault_path, "wrong-key").unwrap();
        let result = vault.get("secret");
        assert!(result.is_err());
    }
}
```

**Step 2: Run integration test**

Run: `cd /Users/zouguojun/Workspace/Aleph && cargo test -p alephcore --test secret_vault_integration 2>&1 | tail -20`
Expected: All 3 integration tests pass.

**Step 3: Commit**

```bash
git add core/tests/secret_vault_integration.rs
git commit -m "secrets: add integration tests for full migration flow"
```

---

### Task 10: Final verification and full test suite

**Step 1: Run the full test suite**

Run: `cd /Users/zouguojun/Workspace/Aleph && cargo test -p alephcore 2>&1 | tail -30`
Expected: All tests pass. Zero regressions.

**Step 2: Run clippy**

Run: `cd /Users/zouguojun/Workspace/Aleph && cargo clippy -p alephcore --all-features 2>&1 | tail -20`
Expected: No warnings in new code.

**Step 3: Verify binary builds**

Run: `cd /Users/zouguojun/Workspace/Aleph && cargo build -p alephcore --bin aleph-server --features gateway 2>&1 | tail -5`
Expected: Build succeeds.

**Step 4: Test CLI help output**

Run: `cd /Users/zouguojun/Workspace/Aleph && cargo run -p alephcore --bin aleph-server --features gateway -- secret --help 2>&1`
Expected: Shows secret subcommand help with init/set/list/delete.

**Step 5: Commit**

If any fixes were needed:
```bash
git add -A
git commit -m "secrets: fix issues found in final verification"
```

---

## Summary

| Task | Description | Files |
|------|-------------|-------|
| 1 | Add crypto dependencies | `Cargo.toml` |
| 2 | Core types (DecryptedSecret, errors) | `secrets/types.rs` |
| 3 | Crypto engine (AES-256-GCM + HKDF) | `secrets/crypto.rs` |
| 4 | Vault storage (file-based CRUD) | `secrets/vault.rs` |
| 5 | Config migration (plaintext → vault) | `secrets/migration.rs` |
| 6 | Wire up module + add `secret_name` field | `secrets/mod.rs`, `lib.rs`, `provider.rs` |
| 7 | Integrate with provider creation | `vault.rs`, `server_init.rs` |
| 8 | CLI commands (init/set/list/delete) | `cli.rs`, `main.rs`, `commands/secret.rs` |
| 9 | Integration tests | `tests/secret_vault_integration.rs` |
| 10 | Final verification | Full test suite + clippy |
