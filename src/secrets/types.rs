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
    /// Create a new `DecryptedSecret` from a string value.
    pub fn new(value: impl Into<String>) -> Self {
        Self {
            value: SecretString::from(value.into()),
        }
    }

    /// Expose the plaintext value. Use sparingly.
    #[must_use]
    pub fn expose(&self) -> &str {
        self.value.expose_secret()
    }
}

impl fmt::Debug for DecryptedSecret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "[REDACTED]")
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
///
/// Both fields look unread from the vault side and neither may be removed on
/// that basis. [`VaultData`] is persisted with **bincode**, a positional format
/// with no field names on the wire: dropping a field changes the byte layout of
/// every [`EncryptedEntry`] already on disk, so an operator's existing vault
/// would fail to deserialize — a data loss, not a cleanup. Removing one is a
/// `VAULT_VERSION` bump plus a migration, never a delete.
///
/// `provider` also has a live writer regardless:
/// `SharedTokenManager::store_secret` (src/gateway/security/shared_token.rs)
/// stamps it on every secret it stores.
#[derive(Clone, Default, Serialize, Deserialize)]
pub struct EntryMetadata {
    /// Human-readable description
    pub description: Option<String>,
    /// Associated provider name (e.g., "anthropic")
    pub provider: Option<String>,
}

/// Serializable vault file format.
#[derive(Serialize, Deserialize)]
pub struct VaultData {
    /// Format version for future migrations
    pub version: u32,
    /// Encrypted entries keyed by name
    pub entries: std::collections::HashMap<String, EncryptedEntry>,
}

impl Default for VaultData {
    fn default() -> Self {
        Self {
            version: 1,
            entries: std::collections::HashMap::new(),
        }
    }
}

/// Secret error types.
///
/// `NotFound` and `InvalidPlaceholder` carry the user-supplied secret name in
/// a side field rather than in their `Display` output: echoing the name
/// verbatim gave a model iterating `{{secret:NAME}}` placeholders a
/// hit-or-miss oracle for vault contents (vault-namespace enumeration).
/// Callers needing the name for triage should reach for [`SecretError::name`]
/// (or `tracing::Span::record("secret_name", …)`); a `Display` consumer must
/// never see it.
#[derive(Debug, thiserror::Error)]
pub enum SecretError {
    #[error("secret not found")]
    NotFound(String),

    #[error("Decryption failed: vault may be corrupted or master key is wrong")]
    DecryptionFailed,

    #[error("Decrypted data is not valid UTF-8")]
    InvalidUtf8,

    #[error("Encryption failed: {0}")]
    EncryptionFailed(String),

    #[error("Vault I/O error: {0}")]
    Io(#[from] std::io::Error),

    #[error("Vault serialization error: {0}")]
    Serialization(String),

    #[error("invalid secret placeholder")]
    InvalidPlaceholder(String),
}

impl SecretError {
    /// Recover the user-supplied secret name carried by `NotFound` /
    /// `InvalidPlaceholder` for triage paths (audit log, operator-only
    /// debug log). Never feed the returned `&str` into a model-facing
    /// surface — that is exactly the path this accessor exists to avoid.
    #[must_use]
    pub fn name(&self) -> Option<&str> {
        match self {
            SecretError::NotFound(name) | SecretError::InvalidPlaceholder(name) => {
                Some(name.as_str())
            }
            _ => None,
        }
    }
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
        assert_eq!(debug, "[REDACTED]");
    }

    #[test]
    fn test_decrypted_secret_display_redacted() {
        let secret = DecryptedSecret::new("sk-ant-api03-xxx");
        let display = format!("{}", secret);
        assert_eq!(display, "[REDACTED]");
        assert!(!display.contains("sk-ant"));
    }

    #[test]
    fn test_vault_data_default() {
        let data = VaultData::default();
        assert_eq!(data.version, 1);
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

    /// Regression for `severed-wire-2026-09-05-modules2 secrets C-1`: the
    /// Display impl of `NotFound` / `InvalidPlaceholder` must NOT include the
    /// user-supplied name, or a model iterating `{{secret:NAME}}` gets a
    /// hit/miss oracle for the vault namespace. The name is still recoverable
    /// for triage via [`SecretError::name`].
    #[test]
    fn test_not_found_display_does_not_echo_name() {
        let err = SecretError::NotFound("ghp_superSecretName42".into());
        let display = format!("{}", err);
        assert!(
            !display.contains("ghp_superSecretName42"),
            "NotFound Display must not echo the name, got: {display}"
        );
        assert_eq!(err.name(), Some("ghp_superSecretName42"));
    }

    #[test]
    fn test_invalid_placeholder_display_does_not_echo_name() {
        let err = SecretError::InvalidPlaceholder("../etc/passwd".into());
        let display = format!("{}", err);
        assert!(
            !display.contains("../etc/passwd"),
            "InvalidPlaceholder Display must not echo the name, got: {display}"
        );
        assert_eq!(err.name(), Some("../etc/passwd"));
    }

    #[test]
    fn test_name_accessor_returns_none_for_other_variants() {
        let err = SecretError::DecryptionFailed;
        assert_eq!(err.name(), None);
    }
}
