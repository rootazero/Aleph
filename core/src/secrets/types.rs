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

    #[error(
        "Master key not configured. Set ALEPH_MASTER_KEY env var or run `aleph secret init`"
    )]
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

    #[error("Provider '{provider}' requires authentication: {message}")]
    ProviderAuthRequired { provider: String, message: String },

    #[error("Provider '{provider}' error: {message}")]
    ProviderError { provider: String, message: String },

    #[error("Access denied for secret '{name}': {reason}")]
    AccessDenied { name: String, reason: String },

    #[error("Provider '{provider}' not configured")]
    ProviderNotFound { provider: String },
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

    #[test]
    fn test_provider_auth_required_error() {
        let err = SecretError::ProviderAuthRequired {
            provider: "1password".into(),
            message: "Session expired".into(),
        };
        assert!(format!("{}", err).contains("1password"));
        assert!(format!("{}", err).contains("authentication"));
    }

    #[test]
    fn test_provider_error() {
        let err = SecretError::ProviderError {
            provider: "1password".into(),
            message: "item not found".into(),
        };
        assert!(format!("{}", err).contains("1password"));
    }

    #[test]
    fn test_access_denied_error() {
        let err = SecretError::AccessDenied {
            name: "bank_password".into(),
            reason: "User denied".into(),
        };
        assert!(format!("{}", err).contains("bank_password"));
    }

    #[test]
    fn test_provider_not_found_error() {
        let err = SecretError::ProviderNotFound {
            provider: "bitwarden".into(),
        };
        assert!(format!("{}", err).contains("bitwarden"));
    }
}
