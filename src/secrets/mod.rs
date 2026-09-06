//! Secret management module
//!
//! Provides encrypted storage for sensitive credentials (API keys, tokens).
//! Uses AES-256-GCM with per-entry HKDF-SHA256 key derivation.

pub mod crypto;
pub mod injection;
pub mod leak_detector;
pub mod placeholder;
pub mod types;
pub mod vault;
pub mod vault_resolver;
pub mod vendor_patterns;
pub mod virtual_key_resolver;

pub use injection::{render_with_secrets, AsyncSecretResolver, InjectedSecret};
pub use leak_detector::{LeakDecision, LeakDetector};
pub use placeholder::{extract_secret_refs, SecretRef};
pub use types::{DecryptedSecret, EntryMetadata, SecretError};
pub use vault::SecretVault;
pub use vault_resolver::VaultSecretResolver;
pub use virtual_key_resolver::VirtualKeyResolver;

const SECRET_NAME_MAX_LEN: usize = 128;

pub fn validate_secret_name(name: &str) -> Result<String, String> {
    let normalized = name.trim();

    if normalized.is_empty() {
        return Err("Secret name cannot be empty".to_string());
    }

    if normalized.len() > SECRET_NAME_MAX_LEN {
        return Err(format!(
            "Secret name must be <= {SECRET_NAME_MAX_LEN} characters"
        ));
    }

    let valid = normalized
        .chars()
        .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '_' | '-' | '.' | ':'));
    if !valid {
        return Err(
            "Secret name can only contain ASCII letters, digits, '_', '-', '.', and ':'"
                .to_string(),
        );
    }

    Ok(normalized.to_string())
}
